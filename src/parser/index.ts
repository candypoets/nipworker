import initWasm, { NostrClient } from './pkg/rust_worker.js';
import wasmUrl from './pkg/rust_worker_bg.wasm?url';

export type InitParserMsg = {
	type: 'init';
	payload: {
		/** Port to communicate with connections worker */
		connectionsPort: MessagePort;
		/** Port to communicate with cache worker */
		cachePort: MessagePort;
		/** Port to communicate with crypto worker */
		cryptoPort: MessagePort;
		/** Port to communicate with main thread (for batched events) */
		mainPort: MessagePort;
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

		const { connectionsPort, cachePort, cryptoPort, mainPort } = msg.payload;

		// Create the Rust worker and start it
		// Note: Rust expects (from_connections, to_cache, from_cache, to_crypto, from_crypto, to_main)
		// Each port is bidirectional, so we pass the same port for send and receive
		const client = await new NostrClient(connectionsPort, cachePort, cachePort, cryptoPort, cryptoPort, mainPort);
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
	}).catch((err) => {
		console.error('[parser worker] Error handling message:', err);
	});
});
