import init, { NipworkerEngine } from './pkg/nipworker_engine.js';
import wasmUrl from './pkg/nipworker_engine_bg.wasm?url';

let pendingMessages: MessageEvent[] = [];
let wasmReady = false;

function handleMessage(event: MessageEvent) {
	const { type, payload } = event.data;
	if (type === 'init' && payload?.port) {
		console.log('[engine worker] creating NipworkerEngine');
		const engine = new NipworkerEngine(payload.port);
		(self as any).__engine = engine;
		self.postMessage({ type: 'ready' });
		console.log('[engine worker] posted ready');
	} else if (type === 'wake') {
		(self as any).__engine?.wake();
	}
}

self.onmessage = (event: MessageEvent) => {
	console.log('[engine worker] received message:', event.data.type);
	if (!wasmReady) {
		pendingMessages.push(event);
	} else {
		handleMessage(event);
	}
};

async function boot() {
	console.log('[engine worker] boot starting');
	try {
		// Using ?url ensures Vite emits the .wasm asset to dist and returns its final URL,
		// which works even when this worker is running from a blob: URL.
		await init(wasmUrl);
		console.log('[engine worker] wasm init complete');
	} catch (e) {
		console.error('[engine worker] wasm init failed:', e);
		throw e;
	}
	wasmReady = true;

	// Process any messages that arrived before init completed
	for (const event of pendingMessages) {
		handleMessage(event);
	}
	pendingMessages = [];
}

boot();
