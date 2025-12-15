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
let instance: any | null = null;

async function ensureWasm() {
	if (!wasmReady) {
		// Using ?url ensures Vite emits the .wasm asset to dist and returns its final URL,
		// which works even when this worker is running from a blob: URL.
		wasmReady = initWasm(wasmUrl);
	}
	return wasmReady;
}

self.addEventListener('message', async (evt: MessageEvent<InitSignerMsg | { type: 'wake' }>) => {
	const msg = evt.data;

	if (msg?.type === 'init') {
		await ensureWasm();

		const { signerRequest, signerResponse, wsSignerRequest, wsSignerResponse } = msg.payload;

		instance = new Signer(signerRequest, signerResponse, wsSignerRequest, wsSignerResponse);

		return;
	}

	// Optional: wake signal; the Rust loops are self-driven, so this is a no-op.
	if (msg?.type === 'wake') {
		return;
	}
});
