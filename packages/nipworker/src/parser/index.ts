/* WASM-based WS worker runtime (dedicated Web Worker, module) */

import initWasm, { NostrClient } from './pkg/rust_worker.js';
import wasmUrl from './pkg/rust_worker_bg.wasm?url';

export type InitParserMsg = {
	type: 'init';
	payload: {
		inRing: SharedArrayBuffer;
		outRing: SharedArrayBuffer;
		ingestRing: SharedArrayBuffer;
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

self.addEventListener('message', async (evt: MessageEvent<InitParserMsg | { type: 'wake' }>) => {
	const msg = evt.data;

	if (msg?.type === 'init') {
		await ensureWasm();

		const { inRing, outRing, ingestRing } = msg.payload;

		instance = new NostrClient(ingestRing, inRing, outRing);

		return;
	}

	// Optional: wake signal; the Rust loops are self-driven, so this is a no-op.
	if (msg?.type === 'wake') {
		return;
	}
});
