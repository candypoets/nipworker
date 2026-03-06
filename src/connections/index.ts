/* WASM-based WS worker runtime (dedicated Web Worker, module) */

import * as flatbuffers from 'flatbuffers';
import initWasm, { WSRust } from './pkg/connections.js';
import wasmUrl from './pkg/connections_bg.wasm?url';
import {
	ConnectionStatus,
	Message,
	MessageType,
	SignerOp,
	SignerRequestT,
	SignerResponse,
	WorkerMessage
} from '../generated/nostr/fb';

export type InitConnectionsMsg = {
	type: 'init';
	payload: {
		/** Port to communicate with main thread (for relay status) */
		mainPort: MessagePort;
		/** Port to communicate with cache worker */
		cachePort: MessagePort;
		/** Port to communicate with parser worker */
		parserPort: MessagePort;
		/** Port to communicate with crypto worker */
		cryptoPort: MessagePort;
		/** Optional relay proxy config */
		proxy?: {
			url: string;
		};
	};
};

let wasmReady: Promise<any> | null = null;
let instance: any | null = null;
let proxyRuntime: ProxyRuntime | null = null;

async function ensureWasm() {
	if (!wasmReady) {
		// Using ?url ensures Vite emits the .wasm asset to dist and returns its final URL,
		// which works even when this worker is running from a blob: URL.
		wasmReady = initWasm(wasmUrl);
	}
	return wasmReady;
}

const AUTH_REQUEST_ID_MASK = 0x8000000000000000n;

function toUint8Array(data: unknown): Uint8Array | null {
	if (data instanceof Uint8Array) return data;
	if (data instanceof ArrayBuffer) return new Uint8Array(data);
	if (ArrayBuffer.isView(data)) {
		const view = data as ArrayBufferView;
		return new Uint8Array(view.buffer, view.byteOffset, view.byteLength);
	}
	if (typeof data === 'string') {
		return new TextEncoder().encode(data);
	}
	return null;
}

function readString(value: unknown): string {
	if (typeof value === 'string') return value;
	if (value instanceof Uint8Array) return new TextDecoder().decode(value);
	if (value && typeof value === 'object' && 'toString' in value) {
		return String(value);
	}
	return '';
}

class ProxyRuntime {
	private ws: WebSocket | null = null;
	private pendingFrames: ArrayBuffer[] = [];
	private authCounter = 1n;

	constructor(
		private readonly proxyUrl: string,
		private readonly mainPort: MessagePort,
		private readonly cachePort: MessagePort,
		private readonly parserPort: MessagePort,
		private readonly cryptoPort: MessagePort
	) {}

	start() {
		this.cachePort.onmessage = (event) => {
			const bytes = toUint8Array(event.data);
			if (!bytes || bytes.byteLength === 0) return;
			this.sendBinary(bytes);
		};

		this.cryptoPort.onmessage = (event) => {
			const bytes = toUint8Array(event.data);
			if (!bytes || bytes.byteLength === 0) return;
			this.handleCryptoResponse(bytes);
		};

		this.openSocket();
	}

	close(subId?: string) {
		if (subId && this.ws?.readyState === WebSocket.OPEN) {
			this.ws.send(JSON.stringify({ type: 'close_sub', subscription_id: subId }));
		}
	}

	private postRelayStatus(status: string, url: string) {
		this.mainPort.postMessage(
			JSON.stringify({
				type: 'relay:status',
				status,
				url
			})
		);
	}

	private openSocket() {
		this.ws = new WebSocket(this.proxyUrl);
		this.ws.binaryType = 'arraybuffer';

		this.ws.onopen = () => {
			this.postRelayStatus('connected', this.proxyUrl);
			const queued = this.pendingFrames.splice(0, this.pendingFrames.length);
			for (const frame of queued) {
				this.ws?.send(frame);
			}
		};

		this.ws.onclose = () => {
			this.postRelayStatus('close', this.proxyUrl);
		};

		this.ws.onerror = () => {
			this.postRelayStatus('failed', this.proxyUrl);
		};

		this.ws.onmessage = async (event: MessageEvent<ArrayBuffer | Blob | string>) => {
			const bytes = await this.messageToBytes(event.data);
			if (!bytes || bytes.byteLength === 0) return;
			this.handleProxyMessage(bytes);
		};
	}

	private async messageToBytes(payload: ArrayBuffer | Blob | string): Promise<Uint8Array | null> {
		if (payload instanceof ArrayBuffer) return new Uint8Array(payload);
		if (payload instanceof Blob) return new Uint8Array(await payload.arrayBuffer());
		if (typeof payload === 'string') return new TextEncoder().encode(payload);
		return null;
	}

	private sendBinary(bytes: Uint8Array) {
		const frame = bytes.slice().buffer;
		if (this.ws?.readyState === WebSocket.OPEN) {
			this.ws.send(frame);
			return;
		}
		this.pendingFrames.push(frame);
	}

	private handleProxyMessage(bytes: Uint8Array) {
		let message: WorkerMessage;
		try {
			message = WorkerMessage.getRootAsWorkerMessage(new flatbuffers.ByteBuffer(bytes));
		} catch {
			return;
		}
		if (
			message.type() === MessageType.ConnectionStatus &&
			message.contentType() === Message.ConnectionStatus
		) {
			const content = message.content(new ConnectionStatus());
			const status = readString(content?.status());
			if (status === 'AUTH') {
				const challenge = readString(content?.message());
				const relay =
					readString(content?.relayUrl()) || readString(message.url()) || readString(message.subId());
				this.forwardAuthChallenge(challenge, relay);
				return;
			}
		}

		this.parserPort.postMessage(bytes, [bytes.buffer]);
	}

	private forwardAuthChallenge(challenge: string, relay: string) {
		if (!challenge || !relay) return;

		const requestId = this.authCounter | AUTH_REQUEST_ID_MASK;
		this.authCounter += 1n;

		const payload = JSON.stringify({
			challenge,
			relay,
			created_at: Date.now()
		});

		const request = new SignerRequestT(requestId, SignerOp.AuthEvent, payload, null, null, null);
		const builder = new flatbuffers.Builder(256);
		builder.finish(request.pack(builder));
		const packet = builder.asUint8Array();
		this.cryptoPort.postMessage(packet, [packet.buffer]);
	}

	private handleCryptoResponse(bytes: Uint8Array) {
		let response: SignerResponse;
		try {
			response = SignerResponse.getRootAsSignerResponse(new flatbuffers.ByteBuffer(bytes));
		} catch {
			return;
		}

		if ((response.requestId() & AUTH_REQUEST_ID_MASK) === 0n) return;

		const resultText = readString(response.result());
		if (!resultText || this.ws?.readyState !== WebSocket.OPEN) return;

		try {
			const parsed = JSON.parse(resultText) as { relay?: string; event?: unknown };
			if (!parsed?.relay || parsed?.event === undefined) return;

			const signedEvent =
				typeof parsed.event === 'string' ? JSON.parse(parsed.event) : parsed.event;

			this.ws.send(
				JSON.stringify({
					type: 'auth_response',
					relay: parsed.relay,
					event: signedEvent
				})
			);
		} catch {
			// Ignore malformed signer response payloads.
		}
	}
}

self.addEventListener(
	'message',
	async (evt: MessageEvent<InitConnectionsMsg | { type: 'wake' } | string>) => {
		const msg = evt.data;

		if (msg?.type === 'init') {
			const { mainPort, cachePort, parserPort, cryptoPort, proxy } = msg.payload;
			if (proxy?.url) {
				proxyRuntime = new ProxyRuntime(proxy.url, mainPort, cachePort, parserPort, cryptoPort);
				proxyRuntime.start();
				return;
			}

			await ensureWasm();

			// Create the Rust worker and start it
			// Note: Rust expects (toMain, fromCache, toParser, fromCrypto, toCrypto)
			instance = new WSRust(mainPort, cachePort, parserPort, cryptoPort, cryptoPort);

			return;
		}

		// Optional: wake signal; the Rust loops are self-driven, so this is a no-op.
		if (msg?.type === 'wake') {
			return;
		}

		if (typeof msg == 'string') {
			proxyRuntime?.close(msg);
			instance?.close(msg);
		}
	}
);
