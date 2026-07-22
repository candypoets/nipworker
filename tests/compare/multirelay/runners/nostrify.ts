import { NPool, NRelay1 } from 'nostrify';

import type { MultiRelayRunner, MultiRunResult } from '../types';
import { RelayTracker } from '../types';

// Nostrify via NPool (its idiomatic multi-relay router) over NRelay1
// connections. NPool.req dedups across relays with a CircularSet of 1000
// event ids — with 2,000 events per relay, ids stream past the 1000-entry
// window, so some cross-relay duplicates DO leak to the consumer (counted by
// the driver). EOSE is aggregate-only (NPool emits EOSE once all relays
// EOSEd). Per-relay socket-open is tracked by hooking each NRelay1's
// websocket-ts socket in the pool's open(). Signature verification is
// overridden (mock relay serves unsigned events); zod message validation
// stays on (no option to disable it).
export function createNostrifyMultiRunner(): MultiRelayRunner {
	let pool: NPool<NRelay1> | null = null;

	return {
		name: 'nostrify',
		perEventWork: [
			'NPool router over one NRelay1 websocket per relay (idiomatic nostrify multi-relay)',
			'zod-schema-validates every incoming relay message (default, cannot be disabled)',
			'dedups across relays via CircularSet(1000) of event ids — leaks duplicates once >1000 ids stream past the window',
			'DEFAULT OVERRIDDEN: schnorr signature verification ON by default; disabled via verifyEvent: () => true (unsigned mock events)',
			'EOSE is aggregate-only in NPool (emitted once ALL relays EOSEd), so per-relay EOSE is all-or-nothing',
			'no content parsing, no persistence'
		],
		async setup() {
			// Created per-run so the socket-open hooks can reach the tracker.
		},
		run(relays, n, _subId, onEvent, timeoutMs): Promise<MultiRunResult> {
			return new Promise((resolve) => {
				const tr = new RelayTracker(relays.length);
				const notes: string[] = [
					'cross-relay dedup: PARTIAL (NPool CircularSet(1000); duplicates leak past the 1000-id window)'
				];
				let done = false;
				const ac = new AbortController();
				const finish = (reason?: string) => {
					if (done) return;
					done = true;
					clearTimeout(timer);
					try {
						ac.abort();
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

				pool = new NPool<NRelay1>({
					open: (url) => {
						const relay = new NRelay1(url, { verifyEvent: () => true });
						try {
							// websocket-ts Websocket is an EventTarget ('open' event).
							(relay.socket as any).addEventListener?.('open', () => tr.markOpen(url));
						} catch {
							/* socket introspection failed; connectAllMs stays -1 */
						}
						return relay;
					},
					reqRouter: (filters) => new Map(relays.map((url) => [url, filters])),
					eventRouter: () => relays
				});

				(async () => {
					try {
						for await (const msg of pool!.req([{ kinds: [1], limit: n }], { signal: ac.signal })) {
							if (msg[0] === 'EVENT') {
								tr.markEvent();
								onEvent(msg[2].id);
							} else if (msg[0] === 'EOSE') {
								// Aggregate: NPool emits EOSE only once every relay sent it.
								for (const url of relays) tr.markEose(url);
								notes.push('EOSE reported aggregate-only (NPool eose = all relays)');
								setTimeout(() => finish(), 300);
							}
						}
						// Iterator ended (CLOSED or abort) before EOSE.
						finish();
					} catch {
						finish();
					}
				})();
			});
		},
		async teardown() {
			try {
				await pool?.close();
			} catch {
				/* best effort */
			}
			pool = null;
		}
	};
}
