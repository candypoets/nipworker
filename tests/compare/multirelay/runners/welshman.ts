import { Pool, SocketEvent } from '@welshman/net';

import type { MultiRelayRunner, MultiRunResult } from '../types';
import { RelayTracker } from '../types';

// Welshman via its Pool/Socket abstraction: one raw REQ per Socket. Events
// consumed from SocketEvent.Receive — the raw socket level has NO dedup, so
// cross-relay duplicates are delivered to the consumer as-is (counted by the
// driver). Note the per-Socket receive TaskQueue (batchSize 20 / batchDelay
// 100ms => ~200 msg/s per relay BY DESIGN); queues are per socket, so
// aggregate throughput scales with relay count.
export function createWelshmanMultiRunner(): MultiRelayRunner {
	const pool = new Pool();

	return {
		name: 'welshman',
		perEventWork: [
			'one raw REQ per Socket (Pool.get(url) per relay)',
			'JSON.parse per EVENT frame, pushed through a per-Socket TaskQueue (batchSize 20 / batchDelay 100ms)',
			'=> consumer delivery throttled to ~200 msg/s PER RELAY by design (queues are per socket, so they run in parallel)',
			'NO dedup at Socket level: cross-relay duplicates ARE delivered to the consumer (counted by the driver)',
			'no signature verification at Socket level (high-level load()/request() APIs default to verifyEvent; not used here)',
			'no content parsing, no persistence'
		],
		async setup() {
			// Nothing: sockets are opened inside run() so connect time stays in
			// the measured window.
		},
		run(relays, n, subId, onEvent, timeoutMs): Promise<MultiRunResult> {
			return new Promise((resolve) => {
				const tr = new RelayTracker(relays.length);
				const notes: string[] = ['cross-relay dedup: NONE at Socket level (duplicates delivered as-is)'];
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

				for (const url of relays) {
					const socket = pool.get(url);
					const onStatus = (status: string) => {
						if (status === 'open') tr.markOpen(url);
					};
					const onMessage = (msg: unknown[]) => {
						if (!Array.isArray(msg) || msg[1] !== subId) return;
						if (msg[0] === 'EVENT') {
							tr.markEvent();
							onEvent((msg[2] as { id: string }).id);
						} else if (msg[0] === 'EOSE') {
							tr.markEose(url);
							if (tr.relaysEosed >= relays.length) setTimeout(() => finish(), 300);
						}
					};
					socket.on(SocketEvent.Status, onStatus);
					socket.on(SocketEvent.Receive, onMessage);
					cleanups.push(() => {
						socket.off(SocketEvent.Status, onStatus);
						socket.off(SocketEvent.Receive, onMessage);
						socket.send(['CLOSE', subId]);
					});
					socket.attemptToOpen();
					socket.send(['REQ', subId, { kinds: [1], limit: n }]);
				}
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
