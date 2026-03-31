import * as flatbuffers from 'flatbuffers';

import {
	ConnectionStatus,
	Message,
	MessageType,
	SignerOp,
	SignerRequestT,
	SignerResponse,
	WorkerMessage
} from '../generated/nostr/fb';

const AUTH_REQUEST_ID_MASK = 0x8000000000000000n;
const TEXT_DECODER = new TextDecoder();
const TEXT_ENCODER = new TextEncoder();
const N46_PREFIX = new Uint8Array([110, 52, 54, 58]); // "n46:"
const AUTH_STATUS = new Uint8Array([65, 85, 84, 72]); // "AUTH"
const N46_PREFIX_TEXT = 'n46:';
const AUTH_STATUS_TEXT = 'AUTH';

function toUint8Array(data: unknown): Uint8Array | null {
	if (data instanceof Uint8Array) return data;
	if (data instanceof ArrayBuffer) return new Uint8Array(data);
	if (ArrayBuffer.isView(data)) {
		const view = data as ArrayBufferView;
		return new Uint8Array(view.buffer, view.byteOffset, view.byteLength);
	}
	if (typeof data === 'string') {
		return TEXT_ENCODER.encode(data);
	}
	return null;
}

function readString(value: unknown): string {
	if (typeof value === 'string') return value;
	if (value instanceof Uint8Array) return TEXT_DECODER.decode(value);
	if (value && typeof value === 'object' && 'toString' in value) {
		return String(value);
	}
	return '';
}

function isAuthStatus(value: string | Uint8Array | null | undefined): boolean {
	if (typeof value === 'string') return value === AUTH_STATUS_TEXT;
	if (!(value instanceof Uint8Array) || value.length !== AUTH_STATUS.length) {
		return false;
	}
	for (let i = 0; i < AUTH_STATUS.length; i++) {
		if (value[i] !== AUTH_STATUS[i]) return false;
	}
	return true;
}

function hasPrefix(
	value: string | Uint8Array | null | undefined,
	prefix: Uint8Array,
	prefixText: string
): boolean {
	if (typeof value === 'string') return value.startsWith(prefixText);
	if (!(value instanceof Uint8Array) || value.length < prefix.length) return false;
	for (let i = 0; i < prefix.length; i++) {
		if (value[i] !== prefix[i]) return false;
	}
	return true;
}

class ProxyRuntime {
	private ws: WebSocket | null = null;
	private pendingFrames: ArrayBuffer[] = [];
	private authCounter = 1n;
	private readonly workerMessageView = new WorkerMessage();
	private readonly connectionStatusView = new ConnectionStatus();
	// Track challenges we've already responded to (NIP-42 dedup)
	private respondedChallenges = new Set<string>();
	// Reconnection state
	private reconnectAttempts = 0;
	private reconnectTimeout: ReturnType<typeof setTimeout> | null = null;
	private readonly maxReconnectDelay = 30000; // Max 30s between reconnection attempts
	private isManualClose = false;

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
		// Clear any pending reconnect timeout
		if (this.reconnectTimeout) {
			clearTimeout(this.reconnectTimeout);
			this.reconnectTimeout = null;
		}

		this.ws = new WebSocket(this.proxyUrl);
		this.ws.binaryType = 'arraybuffer';

		this.ws.onopen = () => {
			this.reconnectAttempts = 0; // Reset on successful connection
			this.isManualClose = false;
			this.postRelayStatus('connected', this.proxyUrl);
			const queued = this.pendingFrames.splice(0, this.pendingFrames.length);
			for (const frame of queued) {
				this.ws?.send(frame);
			}
		};

		this.ws.onclose = () => {
			this.postRelayStatus('close', this.proxyUrl);

			// Only auto-reconnect if not manually closed
			if (!this.isManualClose) {
				this.scheduleReconnect();
			}
		};

		this.ws.onerror = () => {
			this.postRelayStatus('failed', this.proxyUrl);
			// Error typically followed by close, which will trigger reconnect
		};

		this.ws.onmessage = async (event: MessageEvent<ArrayBuffer | Blob | string>) => {
			const bytes = await this.messageToBytes(event.data);
			if (!bytes || bytes.byteLength === 0) return;
			this.handleProxyMessage(bytes);
		};
	}

	/**
	 * Schedule a reconnection attempt with exponential backoff.
	 */
	private scheduleReconnect() {
		if (this.reconnectTimeout) {
			return; // Already scheduled
		}

		// Exponential backoff: 1s, 2s, 4s, 8s, 16s, max 30s
		const delay = Math.min(1000 * Math.pow(2, this.reconnectAttempts), this.maxReconnectDelay);
		this.reconnectAttempts++;

		this.reconnectTimeout = setTimeout(() => {
			this.reconnectTimeout = null;
			if (this.ws?.readyState !== WebSocket.OPEN && this.ws?.readyState !== WebSocket.CONNECTING) {
				this.openSocket();
			}
		}, delay);
	}

	/**
	 * Force an immediate reconnection attempt.
	 * Called on wake signal from main thread (app returning to foreground).
	 */
	forceReconnect() {
		// Clear any pending reconnect
		if (this.reconnectTimeout) {
			clearTimeout(this.reconnectTimeout);
			this.reconnectTimeout = null;
		}

		// Reset reconnect attempts for faster reconnection
		this.reconnectAttempts = 0;

		// Close existing connection if any
		if (this.ws) {
			this.isManualClose = true;
			try {
				this.ws.close();
			} catch {
				// Ignore close errors
			}
			this.ws = null;
		}

		// Open new connection immediately
		this.isManualClose = false;
		this.openSocket();
	}

	private async messageToBytes(payload: ArrayBuffer | Blob | string): Promise<Uint8Array | null> {
		if (payload instanceof ArrayBuffer) return new Uint8Array(payload);
		if (payload instanceof Blob) return new Uint8Array(await payload.arrayBuffer());
		if (typeof payload === 'string') return TEXT_ENCODER.encode(payload);
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
			message = WorkerMessage.getRootAsWorkerMessage(
				new flatbuffers.ByteBuffer(bytes),
				this.workerMessageView
			);
		} catch {
			return;
		}
		const subId = message.subId(flatbuffers.Encoding.UTF8_BYTES);
		const isNip46 = hasPrefix(subId, N46_PREFIX, N46_PREFIX_TEXT);
		if (
			message.type() === MessageType.ConnectionStatus &&
			message.contentType() === Message.ConnectionStatus
		) {
			const content = message.content(this.connectionStatusView);
			const status = content?.status(flatbuffers.Encoding.UTF8_BYTES);
			const relayUrl =
				content?.relayUrl(flatbuffers.Encoding.UTF8_BYTES) ||
				message.url(flatbuffers.Encoding.UTF8_BYTES) ||
				subId;

			if (isAuthStatus(status)) {
				const challenge = readString(content?.message(flatbuffers.Encoding.UTF8_BYTES));
				const relay = readString(relayUrl);
				this.forwardAuthChallenge(challenge, relay);
				return;
			}

			// Forward upstream relay status to main thread
			// (NOTICE, OK, CLOSED, EOSE, etc.)
			const relayUrlText = readString(relayUrl);
			if (relayUrlText && relayUrlText !== this.proxyUrl) {
				this.postRelayStatus(readString(status), relayUrlText);
			}
		}

		const targetPort = isNip46 ? this.cryptoPort : this.parserPort;
		targetPort.postMessage(bytes, [bytes.buffer]);
	}

	private forwardAuthChallenge(challenge: string, relay: string) {
		if (!challenge || !relay) {
			return;
		}

		// Deduplicate - don't respond to same challenge twice
		const challengeKey = `${relay}:${challenge}`;
		if (this.respondedChallenges.has(challengeKey)) {
			return;
		}
		this.respondedChallenges.add(challengeKey);

		// Clean up old challenges (keep last 100)
		if (this.respondedChallenges.size > 100) {
			const toDelete = Array.from(this.respondedChallenges).slice(0, 50);
			toDelete.forEach((k) => this.respondedChallenges.delete(k));
		}

		const requestId = this.authCounter | AUTH_REQUEST_ID_MASK;
		this.authCounter += 1n;

		const payload = JSON.stringify({
			challenge,
			relay,
			created_at: Math.floor(Date.now() / 1000) // Nostr uses seconds, not milliseconds
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

let proxyRuntime: ProxyRuntime | null = null;

self.addEventListener(
	'message',
	(evt: MessageEvent<any | { type: 'wake'; source?: string } | string>) => {
		const msg = evt.data;
		if (msg?.type === 'init') {
			const { mainPort, cachePort, parserPort, cryptoPort, proxy } = msg.payload;
			if (!proxy?.url) {
				return;
			}
			proxyRuntime = new ProxyRuntime(proxy.url, mainPort, cachePort, parserPort, cryptoPort);
			proxyRuntime.start();
			return;
		}

		// Wake signal: app returning from background, force immediate reconnection
		if (msg?.type === 'wake') {
			proxyRuntime?.forceReconnect();
			return;
		}

		if (typeof msg === 'string') {
			proxyRuntime?.close(msg);
		}
	}
);
