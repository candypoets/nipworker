/* WASM-based WS worker runtime (dedicated Web Worker, module) */

import initWasm, { WSRust } from './pkg/connections.js';
import wasmUrl from './pkg/connections_bg.wasm?url';

export type InitConnectionsMsg = {
	type: 'init';
	payload: {
		ws_request: SharedArrayBuffer;
		ws_response: SharedArrayBuffer;
		statusRing: SharedArrayBuffer;
		ws_signer_request?: SharedArrayBuffer;
		ws_signer_response?: SharedArrayBuffer;
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

			const { ws_request, ws_response, statusRing, ws_signer_request, ws_signer_response } =
				msg.payload;

			console.log('[connections] ws_request.len', ws_request.byteLength);
			console.log('[connections] ws_response.len', ws_response.byteLength);
			console.log('[connections] statusRing.len', statusRing.byteLength);
			console.log('[connections] ws_signer_request.present', !!ws_signer_request);
			console.log('[connections] ws_signer_response.present', !!ws_signer_response);

			// Create the Rust worker and start it
			instance = new WSRust(
				ws_request,
				ws_response,
				statusRing,
				ws_signer_request,
				ws_signer_response
			);

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
