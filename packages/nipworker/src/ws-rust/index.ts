/* WASM-based WS worker runtime (dedicated Web Worker, module) */

import initWasm, { WSRust } from './pkg/ws_rust.js';
import wasmUrl from './pkg/ws_rust_bg.wasm?url';

type InitMsg = {
	type: 'init';
	payload: {
		inRings: SharedArrayBuffer[];
		outRings: SharedArrayBuffer[];
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

self.addEventListener('message', async (evt: MessageEvent<InitMsg | { type: 'wake' }>) => {
	const msg = evt.data;

	if (msg?.type === 'init') {
		await ensureWasm();

		const inRings = msg.payload.inRings;
		const outRings = msg.payload.outRings;

		// Create the Rust worker and start it
		instance = new WSRust(inRings as any, outRings as any);
		instance.start();

		return;
	}

	// Optional: wake signal; the Rust loops are self-driven, so this is a no-op.
	if (msg?.type === 'wake') {
		return;
	}
});
