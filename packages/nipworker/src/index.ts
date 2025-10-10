import { NostrManager } from 'src/manager';
import { WSRuntime } from './ws/runtime';
// import { initializeRingHeader } from './ws/ring-buffer';

export * from 'src/manager';

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
	private worker: WSRuntime;

	private hashSubId(sub_id: string): number {
		const target = sub_id.includes('_') ? (sub_id.split('_')[1] ?? '') : sub_id;
		let hash = 0;
		for (let i = 0; i < target.length; i++) {
			hash = (hash << 5) - hash + target.charCodeAt(i);
		}
		return Math.abs(hash) % this.managers.length;
	}

	public createShortId(input: string): string {
		const maxTotalLength = 63;
		const computeHash = (value: string, limit: number): string => {
			let hash = 0;
			for (let i = 0; i < value.length; i++) {
				const char = value.charCodeAt(i);
				hash = (hash << 5) - hash + char;
				hash = hash & hash;
			}
			const short = Math.abs(hash).toString(36);
			return short.substring(0, Math.max(1, limit));
		};

		if (input.includes('_')) {
			const [firstPart, ...rest] = input.split('_');
			const secondPart = rest.join('_');
			const partLimit = Math.max(1, Math.floor((maxTotalLength - 1) / 2));
			const firstShort = computeHash(firstPart ?? '', partLimit);
			const secondShort = computeHash(secondPart ?? '', partLimit);
			const result = `${firstShort}_${secondShort}`;
			return result.length > maxTotalLength ? result.substring(0, maxTotalLength) : result;
		}

		if (input.length < 64) return input;
		return computeHash(input, maxTotalLength);
	}

	constructor(config: any = {}, scale = 1) {
		for (let i = 0; i < scale; i++) {
			const inRing = new SharedArrayBuffer(512 * 1024); // 1MB
			const outRing = new SharedArrayBuffer(2 * 1024 * 1024); // 2MB
			initializeRingHeader(inRing);
			initializeRingHeader(outRing);
			this.inRings.push(inRing);
			this.outRings.push(outRing);

			this.managers.push(
				new NostrManager({
					bufferKey: i.toString(),
					maxBufferSize: 2_000_000,
					inRing,
					outRing
				})
			);
		}

		// Instantiate the main-thread WS runtime instead of a Web Worker
		this.worker = new WSRuntime({
			inRings: this.inRings,
			outRings: this.outRings,
			relayConfig: config
		});
		// const url = new URL('./ws/index.js', import.meta.url);
		// this.worker = new Worker(url, { type: 'module' });

		// this.worker.onerror = (e) => {
		// 	console.error('WS Worker error:', e);
		// };

		// this.worker.postMessage({
		// 	type: 'init',
		// 	payload: {
		// 		inRings: this.inRings,
		// 		outRings: this.outRings,
		// 		relayConfig: config
		// 	}
		// });
		// this.worker.postMessage({ type: 'wake' });
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
