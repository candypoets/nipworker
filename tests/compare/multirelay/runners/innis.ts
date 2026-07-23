import { createRelayPool, type RelayPool } from '@innis/nostr-relay-pool';

import type { MultiRelayRunner, MultiRunResult } from '../types';
import { RelayTracker } from '../types';

// @innis/nostr-relay-pool via RelayPool.subscribeMany — one pool fanning a
// single REQ out to every relay. Pure transport: NO cross-relay dedup (by
// design, "dedup is your event store's job"), so duplicates ARE delivered to
// the consumer and counted by the driver — expect 1600*(R-1) leaked dups,
// same as welshman's raw-socket row. Per-relay EOSE is REAL (onRelayEose
// fires per leg, unlike nostrify/applesauce aggregate-only EOSE); per-relay
// socket-open is tracked via onConnectionChange. No signature verification or
// validation exists in the package, so nothing to override for the unsigned
// mock-relay events.
export function createInnisMultiRunner(): MultiRelayRunner {
	let pool: RelayPool | null = null;

	return {
		name: 'innis',
		perEventWork: [
			'RelayPool.subscribeMany: one websocket per relay, one REQ fanned out to all',
			'JSON.parse per EVENT frame, dispatched straight to onEvent (pure transport)',
			'NO cross-relay dedup BY DESIGN ("dedup is your event store\'s job") — duplicates delivered as-is',
			'NO signature verification or schema validation anywhere in the package (nothing to override)',
			'per-relay latency tracker (20-sample ring) + backoff tracker run in background (no per-event cost)',
			'subscribeMany used with persistent: true — default non-persistent legs arm a 4s soft-EOSE timer',
			'that fires SYNTHETIC onRelayEose while a relay is still streaming (cut off 4-9 legs at x25/x80)',
			'no content parsing, no persistence'
		],
		async setup() {
			// Created per-run so the connection-change hook can reach the tracker.
		},
		run(relays, n, _subId, onEvent, timeoutMs): Promise<MultiRunResult> {
			return new Promise((resolve) => {
				const tr = new RelayTracker(relays.length);
				const notes: string[] = [
					'cross-relay dedup: NONE by design (pure transport; duplicates delivered as-is)'
				];
				let done = false;
				const finish = (reason?: string) => {
					if (done) return;
					done = true;
					clearTimeout(timer);
					try {
						unsubConn();
						sub.unsubscribe();
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

				pool = createRelayPool();
				const unsubConn = pool.onConnectionChange((url, connected) => {
					if (connected) tr.markOpen(url);
				});
				const sub = pool.subscribeMany(
					relays,
					[{ kinds: [1], limit: n }],
					{
						onEvent: (event) => {
							tr.markEvent();
							onEvent(event.id);
						},
						onRelayEose: (url) => {
							tr.markEose(url);
							if (tr.relaysEosed >= relays.length) setTimeout(() => finish(), 300);
						},
						onRelayClosed: (url, reason) => {
							notes.push(`relay ${url} closed early: ${reason}`);
						}
					},
					// persistent: true keeps legs open past EOSE. This matters:
					// in the default non-persistent mode each leg arms a SOFT
					// EOSE timer (suggestedTimeout, default 4s) that fires a
					// SYNTHETIC onRelayEose even if the relay is still
					// connecting/streaming — with 25-80 relays under load that
					// cut off real legs mid-stream. Persistent mode reports
					// only genuine relay EOSEs.
					{ persistent: true }
				);
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
