import init, { NipworkerEngine } from './pkg/nipworker_engine.js';
import wasmUrl from './pkg/nipworker_engine_bg.wasm?url';

let engine: NipworkerEngine | null = null;
let port: MessagePort | null = null;
let wasmReady = false;
const pendingMessages: MessageEvent[] = [];

function forwardEvent(subId: string, data: Uint8Array): void {
	port?.postMessage({ subId, data });
}

function forwardSignerRequest(id: number, op: string, payload: any): void {
	port?.postMessage({ type: 'signer_request', id, op, payload });
}

function handlePortMessage(event: MessageEvent): void {
	if (!engine) return;

	const { type, serializedMessage } = event.data;

	if (serializedMessage) {
		const bytes =
			serializedMessage instanceof Uint8Array
				? serializedMessage
				: new Uint8Array(serializedMessage);
		engine.handleMessage(bytes);
		return;
	}

	if (type === 'signer_response') {
		const { id, result, error } = event.data;
		engine.handleSignerResponse(id, result || '', error || '').catch((e: any) => {
			console.warn('[engine worker] signer response error:', e);
		});
		return;
	}

	if (type === 'set_proxy_signer') {
		const { signerType } = event.data;
		engine.setProxySigner(signerType);
		return;
	}

	if (type === 'wake') {
		engine.wake();
		return;
	}
}

function handleWorkerMessage(event: MessageEvent): void {
	const { type, payload } = event.data;

	if (type === 'init' && payload?.port) {
		port = payload.port;
		port.onmessage = handlePortMessage;

		engine = new NipworkerEngine(forwardEvent, forwardSignerRequest);
		(self as any).__engine = engine;

		self.postMessage({ type: 'ready' });
		console.log('[engine worker] engine ready');
		return;
	}
}

self.onmessage = (event: MessageEvent) => {
	if (!wasmReady) {
		pendingMessages.push(event);
	} else {
		handleWorkerMessage(event);
	}
};

async function boot() {
	console.log('[engine worker] boot starting');
	try {
		await init(wasmUrl);
		console.log('[engine worker] wasm init complete');
	} catch (e) {
		console.error('[engine worker] wasm init failed:', e);
		throw e;
	}
	wasmReady = true;

	for (const event of pendingMessages) {
		handleWorkerMessage(event);
	}
	pendingMessages.length = 0;
}

boot();
