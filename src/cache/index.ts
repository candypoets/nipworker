/* WASM-based WS worker runtime (dedicated Web Worker, module) */

import initWasm, { Caching } from './pkg/cache.js';
import wasmUrl from './pkg/cache_bg.wasm?url';

export type InitCacheMsg = {
	type: 'init';
	payload: {
		ingestRing: SharedArrayBuffer;
		cache_request: SharedArrayBuffer;
		cache_response: SharedArrayBuffer;
		ws_request: SharedArrayBuffer;
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

self.addEventListener('message', async (evt: MessageEvent<InitCacheMsg | { type: 'wake' }>) => {
	const msg = evt.data;

	if (msg?.type === 'init') {
		await ensureWasm();

		const { cache_request, cache_response, ws_request, ingestRing } = msg.payload;

		console.log('[cache] cache_request.len', cache_request.byteLength);
		console.log('[cache] cache_response.len', cache_response.byteLength);
		console.log('[cache] ws_request.len', ws_request.byteLength);
		console.log('[cache] ingestRing.len', ingestRing.byteLength);

		// Create the Rust worker and start it
		instance = new Caching(5 * 1024 * 1024, ingestRing, cache_request, cache_response, ws_request);

		return;
	}

	// Optional: wake signal; the Rust loops are self-driven, so this is a no-op.
	if (msg?.type === 'wake') {
		return;
	}
});
