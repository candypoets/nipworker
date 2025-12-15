import initWasm, { NostrClient } from './pkg/rust_worker.js';
import wasmUrl from './pkg/rust_worker_bg.wasm?url';

export type InitParserMsg = {
	type: 'init';
	payload: {
		ingestRing: SharedArrayBuffer;
		cacheRequest: SharedArrayBuffer;
		cacheResponse: SharedArrayBuffer;
		signerRequest: SharedArrayBuffer;
		signerResponse: SharedArrayBuffer;
		wsResponse: SharedArrayBuffer;
	};
};

let wasmReady: Promise<any> | null = null;
let resolveInstance: ((c: NostrClient) => void) | null = null;
const instanceReady: Promise<NostrClient> = new Promise<NostrClient>((resolve) => {
	resolveInstance = resolve;
});

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

		const { cacheRequest, cacheResponse, signerRequest, signerResponse, wsResponse, ingestRing } =
			msg.payload;

		const client = new NostrClient(
			ingestRing,
			cacheRequest,
			cacheResponse,
			signerRequest,
			signerResponse,
			wsResponse
		);
		// Resolve the deferred so all queued .then handlers can run
		resolveInstance?.(client);
		return;
	}

	// Optional: wake signal; the Rust loops are self-driven, so this is a no-op.
	if (msg?.type === 'wake') {
		return;
	}

	// All non-init messages: await the client promise, then process
	instanceReady.then((c) => {
		c.handle_message(msg);
	});
});
