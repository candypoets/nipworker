/* WASM-based WS worker runtime (dedicated Web Worker, module) */

import initWasm, { Caching } from './pkg/cache.js';
import wasmUrl from './pkg/cache_bg.wasm?url';

export type InitCacheMsg = {
	type: 'init';
	payload: {
		fromParser: MessagePort;
		toConnections: MessagePort;
	};
};

let wasmReady: Promise<any> | null = null;
// eslint-disable-next-line @typescript-eslint/no-unused-vars
let _instance: any | null = null;

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

		const { fromParser, toConnections } = msg.payload;

		console.log('[cache] fromParser port', fromParser);
		console.log('[cache] toConnections port', toConnections);

		// Create the Rust worker and start it
		// TODO: Update Caching::new() to accept MessagePort parameters (US-004)
		_instance = new Caching(fromParser, toConnections);

		return;
	}

	// Optional: wake signal; the Rust loops are self-driven, so this is a no-op.
	if (msg?.type === 'wake') {
		return;
	}
});
