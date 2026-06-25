/* WASM-based parser worker runtime (dedicated Web Worker, module) */

import init, { start_worker, init_tracing } from '../../crates/parser/pkg/nipworker_parser.js';
import wasmUrl from '../../crates/parser/pkg/nipworker_parser_bg.wasm?url';

export type InitParserMsg = {
	type: 'init';
	payload: {
		/** Port to communicate with connections worker */
		connectionsPort: MessagePort;
		/** Port to communicate with cache worker */
		cachePort: MessagePort;
		/** Port to communicate with crypto worker */
		cryptoPort: MessagePort;
		/** Port to communicate with main thread (for commands & batched events) */
		mainPort: MessagePort;
		/** Log level for the Rust WASM worker */
		logLevel?: string;
	};
};

let wasmReady: Promise<any> | null = null;

async function ensureWasm() {
	if (!wasmReady) {
		wasmReady = init({ module_or_path: wasmUrl });
	}
	return wasmReady;
}

self.addEventListener('message', async (evt: MessageEvent<InitParserMsg | { type: 'wake' }>) => {
	const msg = evt.data;

	if (msg?.type === 'init') {
		await ensureWasm();
		const { connectionsPort, cachePort, cryptoPort, mainPort, logLevel } = msg.payload;
		init_tracing(logLevel || 'warn');
		start_worker(mainPort, connectionsPort, cachePort, cryptoPort);
		return;
	}

	// Wake is a no-op; Rust loops are self-driven.
	if (msg?.type === 'wake') {
		return;
	}
});
