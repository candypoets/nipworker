/* WASM-based connections worker runtime (dedicated Web Worker, module) */

import init, {
	start_worker,
	init_tracing,
	wake_all
} from '../../crates/connections/pkg/nipworker_connections.js';

export type InitConnectionsMsg = {
	type: 'init';
	payload: {
		/** Port to communicate with cache worker */
		cachePort: MessagePort;
		/** Port to communicate with parser worker */
		parserPort: MessagePort;
		/** Port to communicate with crypto worker */
		cryptoPort: MessagePort;
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
	async (
		evt: MessageEvent<
			InitConnectionsMsg | { type: 'wake'; source?: string } | { type: 'ping'; id: number } | string
		>
	) => {
		const msg = evt.data;

		if (typeof msg === 'object' && msg !== null && msg.type === 'ping') {
			self.postMessage({ type: 'pong', id: msg.id });
			return;
		}

		if (typeof msg === 'object' && msg !== null && msg.type === 'init') {
			const { parserPort, cachePort, cryptoPort, wasmUrl, logLevel } = (msg as InitConnectionsMsg)
				.payload;
			await ensureWasm(wasmUrl);
			init_tracing(logLevel || 'error');
			start_worker(parserPort, cachePort, cryptoPort);
			return;
		}

		if (typeof msg === 'object' && msg !== null && msg.type === 'wake') {
			await ensureWasm();
			wake_all();
			return;
		}

		// close(subId) is handled by the parser worker (Unsubscribe MainMessage).
		if (typeof msg === 'string') {
			return;
		}
	}
);
