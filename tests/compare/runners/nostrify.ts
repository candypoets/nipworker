import { NRelay1 } from 'nostrify';

import type { ContenderRunner, RunResult } from './types';

// Nostrify via a single NRelay1 connection and its async-iterator req() API.
// Defaults: every relay message is zod-schema-validated (NSchema) and every
// event's schnorr signature is verified (verifyEvent from nostr-tools).
// Verification is overridden because the mock relay serves unsigned events;
// zod message validation stays on (no option to disable it).
export function createNostrifyRunner(): ContenderRunner {
	let relay1: NRelay1 | null = null;

	return {
		name: 'nostrify',
		perEventWork: [
			'zod-schema-validates every incoming relay message (default, cannot be disabled)',
			'matchFilters check per event, delivered through an async iterator (CustomEvent + generator)',
			'DEFAULT OVERRIDDEN: schnorr signature verification per event ON by default;',
			'disabled via `new NRelay1(url, { verifyEvent: () => true })` because mock-relay events are unsigned',
			'no dedup, no content parsing, no persistence'
		],
		async setup(relay: string) {
			// websocket-ts connects immediately on construction; send() queues
			// until the socket is open, so connect time stays in the window.
			relay1 = new NRelay1(relay, { verifyEvent: () => true });
		},
		async run(relay: string, n: number, _subId: string, onEvent: () => void): Promise<RunResult> {
			// NRelay1.req generates its own subscription id (crypto.randomUUID);
			// the mock relay keys its deterministic event stream off the subId,
			// which is fine — each run still gets a full, distinct stream.
			let received = 0;
			const ac = new AbortController();
			let resolveDone!: (r: RunResult) => void;
			const done = new Promise<RunResult>((r) => (resolveDone = r));
			const finish = () => {
				ac.abort();
				resolveDone({ received, rawCount: received, notes: [] });
			};
			(async () => {
				try {
					for await (const msg of relay1!.req([{ kinds: [1], limit: n }], { signal: ac.signal })) {
						if (msg[0] === 'EVENT') {
							received++;
							onEvent();
							if (received >= n) {
								setTimeout(finish, 0);
								return;
							}
						} else if (msg[0] === 'EOSE') {
							setTimeout(finish, 500);
						}
					}
					// Iterator ended (CLOSED or abort) before n events.
					finish();
				} catch (e) {
					finish();
				}
			})();
			return done;
		},
		async teardown() {
			try {
				await (relay1 as any)?.close?.();
			} catch {
				/* best effort */
			}
			relay1 = null;
		}
	};
}
