import { NostrManager } from 'src/manager';
export * from 'src/manager';

export const statusRing = new SharedArrayBuffer(512 * 1024);

// Idempotent header initializer for rings created on the TS side.
// If capacity (u32 at offset 0) is 0, we set it to (byteLength - 32)
// and zero head, tail, and seq. Reserved bytes are cleared as well.
export function initializeRingHeader(buffer: SharedArrayBuffer): void {
	const HEADER = 32;
	const view = new DataView(buffer);
	const total = buffer.byteLength;

	if (total < HEADER) {
		throw new Error(`Ring buffer too small: ${total} bytes`);
	}

	const cap = view.getUint32(0, true);
	if (cap !== 0) {
		// Already initialized; nothing to do.
		return;
	}
	const capacity = total - HEADER;
	if (capacity <= 0) {
		throw new Error(`Invalid ring capacity computed from total=${total}`);
	}

	// Initialize header: capacity, head=0, tail=0, seq=0, reserved=0
	view.setUint32(0, capacity, true); // capacity
	view.setUint32(4, 0, true); // head
	view.setUint32(8, 0, true); // tail
	view.setUint32(12, 0, true); // seq
	// Zero reserved [16..32)
	for (let off = 16; off < 32; off += 4) {
		view.setUint32(off, 0, true);
	}
}

export class NipWorker {
	private inRings: SharedArrayBuffer[] = [];
	private outRings: SharedArrayBuffer[] = [];
	private managers: NostrManager[] = [];
	private worker: Worker;

	constructor(config: any = {}) {
		const wsRequest = new SharedArrayBuffer(5 * 1024 * 1024); // 1MB (ws request)
		const wsResponse = new SharedArrayBuffer(2 * 1024 * 1024); // 2MB (ws response)

		const ingestDBRing = new SharedArrayBuffer(2 * 1024 * 1024);

		initializeRingHeader(wsRequest);
		initializeRingHeader(wsResponse);

		new NostrManager({
			ingestDBRing,
			wsRequest,
			wsResponse
		});

		initializeRingHeader(statusRing);

		// Instantiate the main-thread WS runtime instead of a Web Worker
		// this.worker = new WSRuntime({
		// 	inRings: this.inRings,
		// 	outRings: this.outRings,
		// 	relayConfig: config
		// });
		const url = new URL('./connections/index.js', import.meta.url);
		this.worker = new Worker(url, { type: 'module' });

		this.worker.onerror = (e) => {
			console.error('WS Worker error:', e);
		};

		this.worker.postMessage({
			type: 'init',
			payload: {
				inRings: this.inRings,
				outRings: this.outRings,
				statusRing,
				relayConfig: config
			}
		});
		this.worker.postMessage({ type: 'wake' });
	}

	public cleanup(): void {
		for (const manager of this.managers) {
			manager.cleanup();
		}
	}

	public setSigner(name: string, secretKeyHex: string): void {
		for (const manager of this.managers) {
			manager.setSigner(name, secretKeyHex);
		}
	}

	public getManager(subId: string): NostrManager {
		if (subId?.length > 64) {
			throw new Error('subId cannot exceed 64 characters');
		}
		// subId = this.createShortId(subId);
		const hash = this.hashSubId(subId || '');
		return this.managers[hash] as NostrManager;
	}

	// Called by NostrManager to ensure low-latency pickup by the worker
	public resetInputLoopBackoff(): void {
		// this.worker.postMessage({ type: 'wake' });
		this.worker.wake();
	}
}

export const nipWorker = new NipWorker({});
