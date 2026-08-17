/* WASM-based parser worker runtime (dedicated Web Worker, module) */

import init, { start_worker, init_tracing } from '../../crates/parser/pkg/nipworker_parser.js';

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
		wasmUrl: string;
		/** Log level for the Rust WASM worker */
		logLevel?: string;
	};
};

let wasmReady: Promise<any> | null = null;

async function ensureWasm(wasmUrl?: string) {
	if (!wasmReady) {
		wasmReady = wasmUrl ? init({ module_or_path: wasmUrl }) : init();
	}
	return wasmReady;
}

self.addEventListener(
	'message',
	async (evt: MessageEvent<InitParserMsg | { type: 'wake' } | { type: 'ping'; id: number }>) => {
		const msg = evt.data;

		if (msg?.type === 'ping') {
			self.postMessage({ type: 'pong', id: msg.id });
			return;
		}

		if (msg?.type === 'init') {
			const { connectionsPort, cachePort, cryptoPort, mainPort, wasmUrl, logLevel } = msg.payload;
			await ensureWasm(wasmUrl);
			init_tracing(logLevel || 'error');
			start_worker(mainPort, connectionsPort, cachePort, cryptoPort);
			return;
		}

		// Wake is a no-op; Rust loops are self-driven.
		if (msg?.type === 'wake') {
			return;
		}
	}
);
