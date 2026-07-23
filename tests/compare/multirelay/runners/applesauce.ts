import { RelayPool } from 'applesauce-relay';

import type { MultiRelayRunner, MultiRunResult } from '../types';
import { RelayTracker } from '../types';

// applesauce via RelayPool.request() — the pool's canonical one-shot REQ
// across relays. Unlike the raw Relay.req used in the single-relay suite,
// pool request()/subscription() DEDUPS across relays by default
// (filterDuplicateEvents over an EventMemory, group.js), so cross-relay
// duplicates should NOT leak to the consumer (verified by the driver).
// request() completes once ALL relays sent EOSE, so per-relay EOSE is
// aggregate-only (same caveat as nostrify's NPool). Per-relay socket-open is
// tracked via each Relay's connected$ BehaviorSubject. There is no signature
// verification or schema validation anywhere in applesauce-relay, so nothing
// needs overriding for the unsigned mock-relay events.
export function createApplesauceMultiRunner(): MultiRelayRunner {
	let pool: RelayPool | null = null;

	return {
		name: 'applesauce',
		perEventWork: [
			'RelayPool.request(): one Relay (websocket) per relay, one REQ each',
			'JSON.parse per EVENT frame, rxjs filter/map per message',
			'DEDUPS across relays by default (filterDuplicateEvents over EventMemory) — duplicates should not leak',
			'NO signature verification or schema validation anywhere in applesauce-relay (nothing to override)',
			'EOSE is aggregate-only through request() (completes once ALL relays EOSEd)',
			'no content parsing, no persistence'
		],
		async setup() {
			// Created per-run so the connected$ hooks can reach the tracker.
		},
		run(relays, n, _subId, onEvent, timeoutMs): Promise<MultiRunResult> {
			return new Promise((resolve) => {
				const tr = new RelayTracker(relays.length);
				const notes: string[] = [
					'cross-relay dedup: YES by default (RelayPool.request filterDuplicateEvents/EventMemory)'
				];
				let done = false;
				const cleanups: Array<() => void> = [];
				const finish = (reason?: string) => {
					if (done) return;
					done = true;
					clearTimeout(timer);
					for (const c of cleanups) {
						try {
							c();
						} catch {
							/* best effort */
						}
					}
					if (reason) notes.push(reason);
					resolve(tr.result(notes));
				};
				const timer = setTimeout(
					() => finish(`TIMEOUT after ${timeoutMs}ms (partial results)`),
					timeoutMs
				);

				pool = new RelayPool();

				// Hook per-relay socket-open before the REQ starts the connections.
				for (const url of relays) {
					const relay = pool.relay(url);
					const connSub = relay.connected$.subscribe((connected) => {
						if (connected) tr.markOpen(url);
					});
					cleanups.push(() => connSub.unsubscribe());
				}

				const reqSub = pool.request(relays, [{ kinds: [1], limit: n }]).subscribe({
					next: (event) => {
						tr.markEvent();
						onEvent(event.id);
					},
					error: (err) => {
						notes.push(`request error: ${err?.message || err}`);
						finish();
					},
					complete: () => {
						// request() completes only once every relay sent EOSE.
						for (const url of relays) tr.markEose(url);
						notes.push('EOSE reported aggregate-only (request() completes on all-relay EOSE)');
						setTimeout(() => finish(), 300);
					}
				});
				cleanups.push(() => reqSub.unsubscribe());
			});
		},
		async teardown() {
			try {
				pool?.close();
			} catch {
				/* best effort */
			}
			pool = null;
		}
	};
}
