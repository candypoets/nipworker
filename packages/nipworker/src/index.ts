import { NostrManager } from 'src/manager';
import { initializeRingHeader } from 'src/ws/ring-buffer';

import WsWorker from './ws/index.ts?worker';

// import wsWorker from 'src/ws/index.ts?worker';

export * from 'src/manager';

export class NipWorker {
	private inRings: SharedArrayBuffer[] = [];
	private outRings: SharedArrayBuffer[] = [];
	private managers: NostrManager[] = [];
	private worker: Worker;

	private hashSubId(sub_id: string): number {
		let hash = 0;
		for (let i = 0; i < sub_id.length; i++) {
			hash = (hash << 5) - hash + sub_id.charCodeAt(i);
		}
		return Math.abs(hash) % this.managers.length;
	}

	public createShortId(input: string): string {
		if (input.length < 64) return input;
		let hash = 0;
		for (let i = 0; i < input.length; i++) {
			const char = input.charCodeAt(i);
			hash = (hash << 5) - hash + char;
			hash = hash & hash;
		}
		const shortId = Math.abs(hash).toString(36);
		return shortId.substring(0, 63);
	}

	constructor(config: any = {}, scale = 3) {
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
		subId = this.createShortId(subId);
		const hash = this.hashSubId(subId || '');
		return this.managers[hash] as NostrManager;
	}

	// Called by NostrManager to ensure low-latency pickup by the worker
	public resetInputLoopBackoff(): void {
		this.worker.postMessage({ type: 'wake' });
	}
}

export const nipWorker = new NipWorker({});
