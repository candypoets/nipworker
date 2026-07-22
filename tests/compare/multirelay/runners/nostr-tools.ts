import { SimplePool } from 'nostr-tools';

import type { MultiRelayRunner, MultiRunResult } from '../types';
import { RelayTracker } from '../types';

// nostr-tools v2 SimplePool.subscribeMany across N relays. Dedups across
// relays per subscription (the pool's alreadyHaveEvent shared id set), so no
// duplicates reach onevent. Caveat: oneose fires only once ALL relays have
// sent EOSE (no per-relay EOSE), so relaysEosed is all-or-nothing. Signature
// verification is overridden (mock relay serves unsigned events).
export function createNostrToolsMultiRunner(): MultiRelayRunner {
	let pool: SimplePool | null = null;

	return {
		name: 'nostr-tools',
		perEventWork: [
			'one SimplePool.subscribeMany sub across all relays; dedups by id across relays (shared seen set)',
			'JSON.parse per EVENT frame, matchFilters check, delivered via onevent callback',
			'DEFAULT OVERRIDDEN: schnorr signature verification ON by default; disabled via verifyEvent: () => true (unsigned mock events)',
			'oneose is aggregate-only in nostr-tools (fires when ALL relays EOSEd), so per-relay EOSE is reported as all-or-nothing',
			'connect-all measured via pool.ensureRelay(url) per relay (shared connect promises with the subscription)',
			'no content parsing, no persistence'
		],
		async setup() {
			pool = new SimplePool({ verifyEvent: () => true });
		},
		run(relays, n, subId, onEvent, timeoutMs): Promise<MultiRunResult> {
			return new Promise((resolve) => {
				const tr = new RelayTracker(relays.length);
				const notes: string[] = [
					'cross-relay dedup: YES (pool-level seen-id set per subscription)'
				];
				let done = false;
				let sub: { close: () => void } | null = null;
				const finish = (reason?: string) => {
					if (done) return;
					done = true;
					clearTimeout(timer);
					try {
						sub?.close();
					} catch {
						/* already closed */
					}
					if (reason) notes.push(reason);
					resolve(tr.result(notes));
				};
				const timer = setTimeout(
					() => finish(`TIMEOUT after ${timeoutMs}ms (partial results)`),
					timeoutMs
				);

				// Socket-open tracking: ensureRelay resolves once the relay's
				// websocket is connected. subscribeMany below reuses the same
				// pool connections (shared connect promises), so this adds no
				// extra sockets and stays inside the measured window. The
				// generous connectionTimeout absorbs Chromium's connect-storm
				// latency at 25 simultaneous sockets (the default ~4.4s trips
				// on one random relay otherwise).
				for (const url of relays) {
					pool!
						.ensureRelay(url, { connectionTimeout: 20000 })
						.then(() => tr.markOpen(url))
						.catch(() => notes.push(`failed to connect ${url}`));
				}

				sub = pool!.subscribeMany(relays, { kinds: [1], limit: n }, {
					id: subId,
					maxWait: 20000,
					onevent: (ev) => {
						tr.markEvent();
						onEvent(ev.id);
					},
					oneose: () => {
						// Aggregate: nostr-tools only reports EOSE once every relay sent it.
						for (const url of relays) tr.markEose(url);
						notes.push('EOSE reported aggregate-only (nostr-tools oneose = all relays)');
						setTimeout(() => finish(), 300);
					}
				});
			});
		},
		async teardown() {
			try {
				pool?.destroy();
			} catch {
				/* best effort */
			}
			pool = null;
		}
	};
}
