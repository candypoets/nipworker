import { createRelayPool, type RelayPool, type Subscription } from '@innis/nostr-relay-pool';

import type { ContenderRunner, RunResult } from './types';

// @innis/nostr-relay-pool via RelayPool.subscribe — the pool's single-relay
// REQ path. The package bills itself as "pure transport": no signature
// verification, no schema validation, no dedup ("dedup and persistence are
// your event store's job" — mod.js header), so there is nothing to override
// for the unsigned mock-relay events. A small per-relay latency tracker
// (ring of 20 round-trip samples) runs in the background but does no
// per-event work.
export function createInnisRunner(): ContenderRunner {
	let pool: RelayPool | null = null;

	return {
		name: 'innis',
		perEventWork: [
			'JSON.parse per EVENT frame, dispatched straight to the onEvent callback (pure transport)',
			'NO signature verification, NO schema validation, NO filter matching (none exist in the package)',
			'no dedup (by design: "dedup is your event store\'s job")',
			'per-relay latency tracker keeps a 20-sample ring of round-trip times (no per-event cost)',
			'no content parsing, no persistence'
		],
		async setup() {
			pool = createRelayPool();
		},
		run(relay: string, n: number, _subId: string, onEvent: () => void): Promise<RunResult> {
			return new Promise((resolve) => {
				let received = 0;
				let done = false;
				const notes: string[] = [];
				const finish = () => {
					if (done) return;
					done = true;
					try {
						sub.unsubscribe();
					} catch {
						/* already closed */
					}
					resolve({ received, rawCount: received, notes });
				};
				// Subscribing opens the websocket on demand, so connection setup
				// stays inside the measured window like the others. The pool
				// picks its own wire sub id (not configurable); the mock relay
				// keys its deterministic stream off it, which is fine — each
				// run still gets a full, distinct stream.
				const sub: Subscription = pool!.subscribe(relay, [{ kinds: [1], limit: n }], {
					onEvent: () => {
						received++;
						onEvent();
						if (received >= n) setTimeout(finish, 0);
					},
					onEose: () => {
						setTimeout(finish, 500);
					},
					onClosed: (reason) => {
						notes.push(`closed early: ${reason}`);
						finish();
					}
				});
			});
		},
		async teardown() {
			try {
				pool?.dispose();
			} catch {
				/* best effort */
			}
			pool = null;
		}
	};
}
