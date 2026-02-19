/* WASM-based crypto worker runtime (dedicated Web Worker, module) */

import initWasm, { Crypto } from './pkg/crypto.js';
import wasmUrl from './pkg/crypto_bg.wasm?url';

export type InitCryptoMsg = {
	type: 'init';
	payload: {
		/** Port to communicate with parser worker */
		parserPort: MessagePort;
		/** Port to communicate with connections worker */
		connectionsPort: MessagePort;
		/** Port to communicate with main thread */
		mainPort: MessagePort;
	};
};

const pendingRequests = new Map<number, { resolve: Function; reject: Function }>();
let nextRequestId = 0;

// Expose this to WASM (Nip07Signer)
(self as any).callExtension = (op: string, payload: any): Promise<any> => {
	const id = nextRequestId++;
	return new Promise((resolve, reject) => {
		pendingRequests.set(id, { resolve, reject });
		(self as any).postMessage({ type: 'extension_request', id, op, payload });
	});
};

let wasmReady: Promise<any> | null = null;
let resolveInstance: ((c: Crypto) => void) | null = null;
const instanceReady: Promise<Crypto> = new Promise<Crypto>((resolve) => {
	resolveInstance = resolve;
});

async function ensureWasm() {
	if (!wasmReady) {
		// Using ?url ensures Vite emits the .wasm asset to dist and returns its final URL,
		// which works even when this worker is running from a blob: URL.
		wasmReady = initWasm(wasmUrl);
	}
	return wasmReady;
}

self.addEventListener('message', async (evt: MessageEvent<any>) => {
	const msg = evt.data;

	// Handle response from main thread for NIP-07 extension calls
	if (msg?.type === 'extension_response') {
		const { id, ok, result, error } = msg;
		const pending = pendingRequests.get(id);
		if (pending) {
			pendingRequests.delete(id);
			if (ok) {
				pending.resolve(result);
			} else {
				pending.reject(new Error(error));
			}
		}
		return;
	}

	if (msg?.type === 'init') {
		await ensureWasm();

		const { parserPort, connectionsPort, mainPort } = msg.payload;
		console.log('Initializing Crypto');
		console.log('[crypto] parserPort', parserPort);
		console.log('[crypto] connectionsPort', connectionsPort);
		console.log('[crypto] mainPort', mainPort);

		// Create the Rust worker and start it with MessageChannel ports
		// Parameters: toMain, fromParser, toConnections, fromConnections, toParser
		// Each port is bidirectional, so we pass the same port for send and receive
		const crypto = new Crypto(mainPort, parserPort, connectionsPort, connectionsPort, parserPort);
		// Resolve to deferred so queued handlers can use the instance
		resolveInstance?.(crypto);

		return;
	}

	// Optional: wake signal; Rust loops are self-driven, so this is a no-op.
	if (msg?.type === 'wake') {
		return;
	}

	// All non-init messages: await instance promise, then process
	instanceReady.then(async (c) => {
		try {
			const m: any = msg;
			switch (m?.type) {
				case 'set_private_key': {
					try {
						c.setPrivateKey(m?.payload);
					} catch (e: any) {
						console.error('Error setting private key:', e);
					}
					break;
				}
				case 'set_nip07': {
					c.setNip07();
					break;
				}
				case 'set_nip46_bunker': {
					try {
						const bunkerUrl = m?.payload?.url || m?.payload || '';
						const clientSecret = m?.payload?.clientSecret;
						c.setNip46Bunker(bunkerUrl, clientSecret);
					} catch (e: any) {
						console.error('Error setting NIP-46 with bunker URL:', e);
						(self as any).postMessage({
							id: m.id,
							type: 'response',
							op: 'set_nip46_bunker',
							ok: false,
							error: e.message
						});
					}
					break;
				}
				case 'set_nip46_qr': {
					try {
						const nostrconnectUrl = m?.payload?.url || m?.payload || '';
						const clientSecret = m?.payload?.clientSecret;
						c.setNip46QR(nostrconnectUrl, clientSecret);
					} catch (e: any) {
						console.error('Error setting NIP-46 with QR code:', e);
						(self as any).postMessage({
							id: m.id,
							type: 'response',
							op: 'set_nip46_qr',
							ok: false,
							error: e.message
						});
					}
					break;
				}

				case 'connect': {
					try {
						const res = await c.connectDirect();
						(self as any).postMessage({
							id: m.id,
							type: 'response',
							op: 'connect',
							ok: true,
							result: res
						});
					} catch (e: any) {
						console.error('Error connecting NIP-46:', e);
						(self as any).postMessage({
							id: m.id,
							type: 'response',
							op: 'connect',
							ok: false,
							error: e.message
						});
					}
					break;
				}

				case 'get_pubkey': {
					try {
						const pk = await c.getPublicKeyDirect();
						(self as any).postMessage({
							id: m.id,
							type: 'response',
							op: 'get_pubkey',
							ok: true,
							result: pk
						});
					} catch (e: any) {
						console.error('Error getting public key:', e);
					}
					break;
				}

				case 'sign_event': {
					try {
						const signed = await c.signEvent(m?.payload);
						(self as any).postMessage({
							id: m.id,
							type: 'response',
							op: 'sign_event',
							ok: true,
							result: signed
						});
					} catch (e: any) {
						console.error('Error signing event:', e);
					}
					break;
				}

				case 'clear_signer': {
					c.clearSigner();
					break;
				}

				default: {
					console.error('Unknown message type:', m.type);
				}
			}
		} catch (e: any) {
			if ((msg as any)?.id !== undefined) {
				console.error('Error processing message:', e);
			}
		}
	});
});
