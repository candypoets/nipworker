import { Pool, SocketEvent } from '@welshman/net';

import type { ContenderRunner, RunResult } from './types';

// Welshman via its Pool/Socket abstraction: a raw REQ over a Socket, events
// consumed from SocketEvent.Receive. Note the Socket's receive TaskQueue:
// batchSize=20 messages per batchDelay=100ms, i.e. consumer delivery is
// throttled to ~200 events/sec by design. That is welshman's default delivery
// pipeline (the high-level load()/request() APIs sit on the same queue), so
// the numbers include it.
export function createWelshmanRunner(): ContenderRunner {
	const pool = new Pool();

	return {
		name: 'welshman',
		perEventWork: [
			'JSON.parse per EVENT frame, pushed through a TaskQueue (batchSize 20 / batchDelay 100ms)',
			'=> consumer delivery throttled to ~200 msg/s BY DESIGN at Socket level',
			'no signature verification at Socket level (the high-level load()/request() APIs default to',
			'isEventValid = verifyEvent; not used here because mock-relay events are unsigned)',
			'no dedup, no content parsing, no persistence'
		],
		async setup(relay: string) {
			// Do NOT wait for the socket to open: the send queue is paused until
			// open, so the REQ issued in run() rides through connection setup —
			// keeping connect time inside the measured window like the others.
			pool.get(relay).attemptToOpen();
		},
		run(relay: string, n: number, subId: string, onEvent: () => void): Promise<RunResult> {
			return new Promise((resolve) => {
				const socket = pool.get(relay);
				let received = 0;
				let done = false;
				const finish = () => {
					if (done) return;
					done = true;
					socket.off(SocketEvent.Receive, onMessage);
					socket.send(['CLOSE', subId]);
					resolve({ received, rawCount: received, notes: [] });
				};
				const onMessage = (msg: unknown[]) => {
					if (!Array.isArray(msg) || msg[1] !== subId) return;
					if (msg[0] === 'EVENT') {
						received++;
						onEvent();
						if (received >= n) setTimeout(finish, 0);
					} else if (msg[0] === 'EOSE') {
						setTimeout(finish, 500);
					}
				};
				socket.on(SocketEvent.Receive, onMessage);
				socket.send(['REQ', subId, { kinds: [1], limit: n }]);
			});
		},
		async teardown() {
			try {
				pool.clear();
			} catch {
				/* best effort */
			}
		}
	};
}
