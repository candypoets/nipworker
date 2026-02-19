/* WASM-based WS worker runtime (dedicated Web Worker, module) */

import initWasm, { Caching } from './pkg/cache.js';
import wasmUrl from './pkg/cache_bg.wasm?url';

export type InitCacheMsg = {
	type: 'init';
	payload: {
		/** Port to communicate with parser worker */
		parserPort: MessagePort;
		/** Port to communicate with connections worker */
		connectionsPort: MessagePort;
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

		const { parserPort, connectionsPort } = msg.payload;

		console.log('[cache] parserPort', parserPort);
		console.log('[cache] connectionsPort', connectionsPort);

		// Create the Rust worker and start it
		// Default buffer size: 1MB for general ring buffer usage
		const maxBufferSize = 1024 * 1024;
		// Note: Rust expects (maxBufferSize, from_parser, to_connections)
		_instance = new Caching(maxBufferSize, parserPort, connectionsPort);

		return;
	}

	// Optional: wake signal; the Rust loops are self-driven, so this is a no-op.
	if (msg?.type === 'wake') {
		return;
	}
});
