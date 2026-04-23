/* WASM-based crypto worker runtime (dedicated Web Worker, module) */

import init, { start_worker } from '../../crates/crypto/pkg/nipworker_crypto.js';
import wasmUrl from '../../crates/crypto/pkg/nipworker_crypto_bg.wasm?url';

export type InitCryptoMsg = {
	type: 'init';
	payload: {
		/** Port to communicate with parser worker */
		parserPort: MessagePort;
		/** Port to communicate with connections worker */
		connectionsPort: MessagePort;
		/** Port to communicate with main thread */
		mainPort: MessagePort;
	};
};

const pendingRequests = new Map<number, { resolve: Function; reject: Function }>();
let nextRequestId = 0;

// Expose this to WASM (Nip07Signer)
(self as any).callExtension = (op: string, payload: any): Promise<any> => {
	const id = nextRequestId++;
	return new Promise((resolve, reject) => {
		pendingRequests.set(id, { resolve, reject });
		(self as any).postMessage({ type: 'extension_request', id, op, payload });
	});
};

let wasmReady: Promise<any> | null = null;

async function ensureWasm() {
	if (!wasmReady) {
		wasmReady = init({ module_or_path: wasmUrl });
	}
	return wasmReady;
}

self.addEventListener('message', async (evt: MessageEvent<any>) => {
	const msg = evt.data;

	// Handle response from main thread for NIP-07 extension calls
	if (msg?.type === 'extension_response') {
		const { id, ok, result, error } = msg;
		const pending = pendingRequests.get(id);
		if (pending) {
			pendingRequests.delete(id);
			if (ok) {
				pending.resolve(result);
			} else {
				pending.reject(new Error(error));
			}
		}
		return;
	}

	if (msg?.type === 'init') {
		await ensureWasm();
		const { parserPort, connectionsPort, mainPort } = msg.payload;
		start_worker(mainPort, parserPort, connectionsPort);
		return;
	}

	// Wake is a no-op; Rust loops are self-driven.
	if (msg?.type === 'wake') {
		return;
	}
});
