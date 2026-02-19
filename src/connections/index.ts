/* WASM-based WS worker runtime (dedicated Web Worker, module) */

import initWasm, { WSRust } from './pkg/connections.js';
import wasmUrl from './pkg/connections_bg.wasm?url';

export type InitConnectionsMsg = {
	type: 'init';
	payload: {
		statusRing: SharedArrayBuffer;
		fromCache: MessagePort;
		toParser: MessagePort;
		fromCrypto: MessagePort;
		toCrypto: MessagePort;
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

			const { statusRing, fromCache, toParser, fromCrypto, toCrypto } = msg.payload;

			console.log('[connections] statusRing.len', statusRing.byteLength);
			console.log('[connections] fromCache port', fromCache);
			console.log('[connections] toParser port', toParser);
			console.log('[connections] fromCrypto port', fromCrypto);
			console.log('[connections] toCrypto port', toCrypto);

			// Create the Rust worker and start it
			instance = new WSRust(statusRing, fromCache, toParser, fromCrypto, toCrypto);

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
