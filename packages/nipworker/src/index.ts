import { NostrManager } from 'src/manager';
import { initializeRingHeader } from 'src/ws/ring-buffer';

export * from 'src/manager';

export class NipWorker {
	private inRings: SharedArrayBuffer[] = [];
	private outRings: SharedArrayBuffer[] = [];
	private managers: NostrManager[] = [];
	private worker: Worker;

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

	constructor(config: any = {}, scale = 2) {
		for (let i = 0; i < scale; i++) {
			const inRing = new SharedArrayBuffer(1 * 1024 * 1024); // 1MB
			const outRing = new SharedArrayBuffer(5 * 1024 * 1024); // 5MB
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
		const url = /* @vite-ignore */ new URL('./ws/index.js', import.meta.url);
		this.worker = new Worker(url, { type: 'module' });

		this.worker.onerror = (e) => {
			console.error('WS Worker error:', e);
		};
		// this.worker = new Worker(workerUrl, { type: 'module' });

		console.log('ok', url, this.worker);

		this.worker.postMessage({
			type: 'init',
			payload: {
				inRings: this.inRings,
				outRings: this.outRings,
				relayConfig: config
			}
		});
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
		this.worker.postMessage({ type: 'wake' });
	}
}

export const nipWorker = new NipWorker({});
