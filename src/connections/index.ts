/* WASM-based connections worker runtime (dedicated Web Worker, module) */

import init, { start_worker, init_tracing } from '../../crates/connections/pkg/nipworker_connections.js';
import wasmUrl from '../../crates/connections/pkg/nipworker_connections_bg.wasm?url';

export type InitConnectionsMsg = {
	type: 'init';
	payload: {
		/** Port to communicate with main thread (for relay status) */
		mainPort: MessagePort;
		/** Port to communicate with cache worker */
		cachePort: MessagePort;
		/** Port to communicate with parser worker */
		parserPort: MessagePort;
		/** Port to communicate with crypto worker */
		cryptoPort: MessagePort;
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

self.addEventListener(
	'message',
	async (evt: MessageEvent<InitConnectionsMsg | { type: 'wake'; source?: string } | string>) => {
		const msg = evt.data;

		if (typeof msg === 'object' && msg !== null && msg.type === 'init') {
			const { parserPort, cachePort, cryptoPort, logLevel } = (msg as InitConnectionsMsg).payload;
			await ensureWasm();
			init_tracing(logLevel || 'warn');
			start_worker(parserPort, cachePort, cryptoPort);
			return;
		}

		// Wake is a no-op in the new architecture.
		if (typeof msg === 'object' && msg !== null && msg.type === 'wake') {
			return;
		}

		// close(subId) is handled by the parser worker (Unsubscribe MainMessage).
		if (typeof msg === 'string') {
			return;
		}
	}
);
