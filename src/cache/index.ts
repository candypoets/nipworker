/* WASM-based cache worker runtime (dedicated Web Worker, module) */

import init, { start_worker } from '../../crates/cache/pkg/nipworker_cache.js';
import wasmUrl from '../../crates/cache/pkg/nipworker_cache_bg.wasm?url';

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

async function ensureWasm() {
	if (!wasmReady) {
		wasmReady = init({ module_or_path: wasmUrl });
	}
	return wasmReady;
}

self.addEventListener('message', async (evt: MessageEvent<InitCacheMsg | { type: 'wake' }>) => {
	const msg = evt.data;

	if (msg?.type === 'init') {
		await ensureWasm();
		const { parserPort, connectionsPort } = msg.payload;
		start_worker(parserPort, connectionsPort);
		return;
	}

	// Wake is a no-op; Rust loops are self-driven.
	if (msg?.type === 'wake') {
		return;
	}
});
