import { Relay } from 'applesauce-relay';

import type { ContenderRunner, RunResult } from './types';

// applesauce (hzrd149) via the socket-level Relay class: one raw REQ over a
// single Relay connection, events consumed from the req() observable. This is
// the same layer used for welshman (raw REQ -> delivery), NOT the high-level
// EventStore/model layer. Notably, Relay performs NO signature verification
// and NO schema validation at all (no verifyEvent exists in applesauce-relay),
// so unlike nostr-tools/NDK/nostrify there is nothing to override for the
// unsigned mock-relay events.
export function createApplesauceRunner(): ContenderRunner {
	let relay: Relay | null = null;

	return {
		name: 'applesauce',
		perEventWork: [
			'JSON.parse per EVENT frame, rxjs filter/map per message (Relay.req observable pipeline)',
			'NO signature verification anywhere in applesauce-relay (nothing to override; unsigned mock events fine)',
			'NO schema validation, no dedup at Relay level',
			'(dedup only exists one layer up: RelayPool request()/subscription() via filterDuplicateEvents)',
			'no content parsing, no persistence'
		],
		async setup(relayUrl: string) {
			relay = new Relay(relayUrl);
		},
		run(relayUrl: string, n: number, subId: string, onEvent: () => void): Promise<RunResult> {
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
				// Subscribing to req() opens the websocket on demand, so connection
				// setup stays inside the measured window like the others.
				const sub = relay!.req([{ kinds: [1], limit: n }], { id: subId }).subscribe({
					next: (msg) => {
						if (msg.type === 'EVENT') {
							received++;
							onEvent();
							if (received >= n) setTimeout(finish, 0);
						} else if (msg.type === 'EOSE') {
							setTimeout(finish, 500);
						}
					},
					error: (err) => {
						notes.push(`req error: ${err?.message || err}`);
						finish();
					},
					complete: () => finish()
				});
			});
		},
		async teardown() {
			try {
				(relay as any)?.close?.();
			} catch {
				/* best effort */
			}
			relay = null;
		}
	};
}
