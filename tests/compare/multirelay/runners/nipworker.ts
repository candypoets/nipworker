import type { MultiRelayRunner, MultiRunResult } from '../types';
import { RelayTracker } from '../types';

// nipworker: 4-worker WASM pipeline (connections -> parser -> cache, crypto
// idle), single subscription fanned out to all relays. The parser worker
// dedups by event id across relays before batching to the main thread, so no
// duplicates reach the consumer callback. Per-relay EOSE arrives at main as
// ConnectionStatus WorkerMessages; socket-open state via useRelayStatus.
export function createNipworkerMultiRunner(): MultiRelayRunner {
	let manager: any = null;
	let useSubscription: any = null;
	let useRelayStatus: any = null;
	let isParsedEvent: any = null;
	let isConnectionStatus: any = null;

	return {
		name: 'nipworker',
		perEventWork: [
			'4 WASM workers (connections/parser/cache/crypto); FlatBuffers IPC over MessageChannel',
			'one subscription across all relays; connections worker opens one socket per relay',
			'parses JSON -> typed ParsedEvent, kind-specific parsing, dedups by id across relays in the parser worker (10k id ring)',
			'builds FlatBuffer WorkerMessage per event, batches to main thread',
			'save-to-IndexedDB pipe runs in cache worker (skipCache only skips cache *reads*)',
			'NO schnorr signature verification on ingest (same as single-relay runner)',
			'heap numbers cover the MAIN THREAD only; 4 worker heaps + WASM linear memory are not visible to performance.memory'
		],
		async setup(_relays: string[]) {
			const idx = await import('../../../../src/index');
			const hooks = await import('../../../../src/hooks');
			useSubscription = hooks.useSubscription;
			useRelayStatus = hooks.useRelayStatus;
			isParsedEvent = hooks.isParsedEvent;
			isConnectionStatus = hooks.isConnectionStatus;
			manager = idx.createNostrManager();
			idx.setManager(manager);
			// Give the workers a moment to boot (same as the single-relay runner).
			await new Promise((r) => setTimeout(r, 1000));
		},
		run(relays, n, subId, onEvent, timeoutMs): Promise<MultiRunResult> {
			return new Promise((resolve) => {
				const tr = new RelayTracker(relays.length);
				const notes: string[] = ['cross-relay dedup: YES (parser-worker id ring, before batching to main)'];
				const relaySet = new Set(relays);
				let done = false;
				let unsub: (() => void) | null = null;
				let stopStatus: (() => void) | null = null;
				const finish = (reason?: string) => {
					if (done) return;
					done = true;
					clearTimeout(timer);
					try {
						unsub?.();
					} catch {
						/* already closed */
					}
					try {
						stopStatus?.();
					} catch {
						/* best effort */
					}
					if (reason) notes.push(reason);
					resolve(tr.result(notes));
				};
				const timer = setTimeout(
					() => finish(`TIMEOUT after ${timeoutMs}ms (partial results)`),
					timeoutMs
				);

				// Socket-open tracking (fires immediately for already-known statuses,
				// then on every 'relay:status' event).
				stopStatus = useRelayStatus((status: string, url: string) => {
					if (status === 'connected' && relaySet.has(url)) tr.markOpen(url);
				});

				// closeOnEose stays OFF here: with multiple relays the per-relay EOSE
				// statuses arrive interleaved, and we unsubscribe manually once every
				// relay has EOSEd.
				unsub = useSubscription(
					subId,
					[{ kinds: [1], limit: n, relays }],
					(msg: unknown) => {
						const m = msg as any;
						const ev = isParsedEvent(m);
						if (ev) {
							tr.markEvent();
							onEvent(ev.id());
							return;
						}
						const cs = isConnectionStatus(m);
						if (cs && cs.status() === 'EOSE') {
							tr.markEose(m.url?.() ?? `eose#${tr.relaysEosed}`);
							if (tr.relaysEosed >= relays.length) setTimeout(() => finish(), 300);
						}
					},
					{ closeOnEose: false, bytesPerEvent: 8192, skipCache: true }
				);
			});
		},
		async teardown() {
			try {
				manager?.destroy?.();
			} catch {
				/* best effort */
			}
			manager = null;
		}
	};
}
