/* WASM-based WS worker runtime (dedicated Web Worker, module) */

import initWasm, { WSRust } from './pkg/connections.js';
import wasmUrl from './pkg/connections_bg.wasm?url';

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

self.addEventListener(
	'message',
	async (evt: MessageEvent<InitConnectionsMsg | { type: 'wake' }>) => {
		const msg = evt.data;

		if (msg?.type === 'init') {
			await ensureWasm();

			const { mainPort, cachePort, parserPort, cryptoPort } = msg.payload;

			// Create the Rust worker and start it
			// Note: Rust expects (toMain, fromCache, toParser, fromCrypto, toCrypto)
			instance = new WSRust(mainPort, cachePort, parserPort, cryptoPort, cryptoPort);

			return;
		}

		// Optional: wake signal; the Rust loops are self-driven, so this is a no-op.
		if (msg?.type === 'wake') {
			return;
		}

		if (typeof msg == 'string') {
			instance.close(msg);
		}
	}
);
