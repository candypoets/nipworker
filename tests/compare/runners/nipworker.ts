import type { ContenderRunner, RunResult } from './types';

// nipworker: 4-worker WASM pipeline (connections -> parser -> cache, crypto idle).
// Mirror of tests/bench/bench.ts throughput phase: skipCache + closeOnEose.
export function createNipworkerRunner(): ContenderRunner {
	let manager: any = null;
	let useSubscription: any = null;
	let isParsedEvent: any = null;

	return {
		name: 'nipworker',
		perEventWork: [
			'4 WASM workers (connections/parser/cache/crypto); FlatBuffers IPC over MessageChannel',
			'parses JSON -> typed ParsedEvent, kind-specific parsing, dedups (10k id ring)',
			'builds FlatBuffer WorkerMessage per event, batches to main thread',
			'save-to-IndexedDB pipe runs in cache worker (skipCache only skips cache *reads*)',
			'NO schnorr signature verification on ingest (verify_event_signature exists in crypto worker but is not wired into the pipeline)',
			'heap numbers cover the MAIN THREAD only; 4 worker heaps + WASM linear memory are not visible to performance.memory'
		],
		async setup(_relay: string) {
			const idx = await import('../../../src/index');
			const hooks = await import('../../../src/hooks');
			useSubscription = hooks.useSubscription;
			isParsedEvent = hooks.isParsedEvent;
			manager = idx.createNostrManager();
			idx.setManager(manager);
			// Give the workers a moment to boot (same as tests/bench/bench.ts).
			await new Promise((r) => setTimeout(r, 1000));
		},
		run(relay: string, n: number, subId: string, onEvent: () => void): Promise<RunResult> {
			return new Promise((resolve) => {
				let received = 0;
				let done = false;
				const finish = () => {
					if (done) return;
					done = true;
					try {
						unsub();
					} catch {
						/* already closed */
					}
					resolve({ received, rawCount: received, notes: [] });
				};
				const unsub = useSubscription(
					subId,
					[{ kinds: [1], limit: n, relays: [relay] }],
					(msg: unknown) => {
						if (!isParsedEvent(msg)) return;
						received++;
						onEvent();
						if (received >= n) {
							// Let the callback loop drain before unsubscribing (same as bench).
							setTimeout(finish, 0);
						}
					},
					{ closeOnEose: true, bytesPerEvent: 8192, skipCache: true }
				);
			});
		},
		async teardown() {
			try {
				manager?.destroy?.();
			} catch {
				/* best effort */
			}
		}
	};
}
