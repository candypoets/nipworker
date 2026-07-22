import { SimplePool } from 'nostr-tools';

import type { ContenderRunner, RunResult } from './types';

// nostr-tools v2 SimplePool. Default verifies every event's schnorr signature
// (super({ verifyEvent, ... })) — overridden here because the mock relay
// serves synthetic unsigned events. Dedups per subscription across relays.
export function createNostrToolsRunner(): ContenderRunner {
	let pool: SimplePool | null = null;

	return {
		name: 'nostr-tools',
		perEventWork: [
			'JSON.parse per EVENT frame, matchFilters check, delivered via onevent callback',
			'dedups per subscription across relays (single relay here, so no-op)',
			'DEFAULT OVERRIDDEN: schnorr signature verification per event is ON by default (verifyEvent);',
			'disabled via `new SimplePool({ verifyEvent: () => true })` because mock-relay events are unsigned',
			'no content parsing, no persistence'
		],
		async setup() {
			pool = new SimplePool({ verifyEvent: () => true });
		},
		run(relay: string, n: number, subId: string, onEvent: () => void): Promise<RunResult> {
			return new Promise((resolve) => {
				let received = 0;
				let done = false;
				const finish = () => {
					if (done) return;
					done = true;
					try {
						sub.close();
					} catch {
						/* already closed */
					}
					resolve({ received, rawCount: received, notes: [] });
				};
				const sub = pool!.subscribeMany([relay], { kinds: [1], limit: n }, {
					id: subId,
					onevent: () => {
						received++;
						onEvent();
						if (received >= n) setTimeout(finish, 0);
					},
					oneose: () => {
						// Drain anything still in flight, then finish.
						setTimeout(finish, 500);
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
