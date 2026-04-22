import init, { NipworkerEngine } from './pkg/nipworker_engine.js';
import wasmUrl from './pkg/nipworker_engine_bg.wasm?url';

let engine: NipworkerEngine | null = null;
let port: MessagePort | null = null;
let wasmReady = false;
const pendingMessages: MessageEvent[] = [];

function forwardEvent(subId: string, data: Uint8Array): void {
	port?.postMessage({ subId, data });
}

const pendingExtensionRequests = new Map<number, { resolve: Function; reject: Function }>();
let nextExtensionId = 0;

(self as any).callExtension = (op: string, payload: any): Promise<any> => {
	const id = nextExtensionId++;
	return new Promise((resolve, reject) => {
		pendingExtensionRequests.set(id, { resolve, reject });
		port?.postMessage({ type: 'extension_request', id, op, payload });
	});
};

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

	if (type === 'extension_response') {
		const { id, ok, result, error } = event.data;
		const pending = pendingExtensionRequests.get(id);
		if (pending) {
			pendingExtensionRequests.delete(id);
			ok ? pending.resolve(result) : pending.reject(new Error(error));
		}
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

		engine = new NipworkerEngine(forwardEvent);
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
