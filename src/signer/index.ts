/* WASM-based WS worker runtime (dedicated Web Worker, module) */

import initWasm, { Signer } from './pkg/signer.js';
import wasmUrl from './pkg/signer_bg.wasm?url';

export type InitSignerMsg = {
	type: 'init';
	payload: {
		wsSignerRequest: SharedArrayBuffer;
		wsSignerResponse: SharedArrayBuffer;
		signerRequest: SharedArrayBuffer;
		signerResponse: SharedArrayBuffer;
	};
};

let wasmReady: Promise<any> | null = null;
let resolveInstance: ((s: Signer) => void) | null = null;
const instanceReady: Promise<Signer> = new Promise<Signer>((resolve) => {
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

	if (msg?.type === 'init') {
		await ensureWasm();

		const { signerRequest, signerResponse, wsSignerRequest, wsSignerResponse } = msg.payload;
		console.log('Initializing Signer');
		const signer = new Signer(signerRequest, signerResponse, wsSignerRequest, wsSignerResponse);
		// Resolve the deferred so queued handlers can use the instance
		resolveInstance?.(signer);

		return;
	}

	// Optional: wake signal; the Rust loops are self-driven, so this is a no-op.
	if (msg?.type === 'wake') {
		return;
	}

	// All non-init messages: await the instance promise, then process
	instanceReady.then(async (s) => {
		try {
			const m: any = msg;
			switch (m?.type) {
				case 'set_private_key': {
					try {
						s.setPrivateKey(m?.payload);
					} catch (e: any) {
						console.error('Error setting private key:', e);
					}
					break;
				}
				case 'set_nip07': {
					s.setNip07();
					break;
				}
				case 'set_nip46_bunker': {
					try {
						const bunkerUrl = m?.payload?.url || m?.payload || '';
						const clientSecret = m?.payload?.clientSecret;
						s.setNip46Bunker(bunkerUrl, clientSecret);
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
						s.setNip46QR(nostrconnectUrl, clientSecret);
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
						const res = await s.connectDirect();
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
						const pk = await s.getPublicKeyDirect();
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
						const signed = await s.signEvent(m?.payload);
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
