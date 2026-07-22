import NDK from '@nostr-dev-kit/ndk';

import type { MultiRelayRunner, MultiRunResult } from '../types';
import { RelayTracker } from '../types';

// NDK 3.x with all relays as explicitRelayUrls and a single subscription.
// Dedups by event id across relays (suppressed duplicates fire event:dup
// instead of event). EOSE is aggregate-only (emitted once all relays EOSEd).
// Verification/validation overridden (unsigned mock events).
export function createNdkMultiRunner(): MultiRelayRunner {
	let ndk: NDK | null = null;

	return {
		name: 'ndk',
		perEventWork: [
			'one NDK subscription fanned out to all relays by the pool',
			'JSON.parse per EVENT frame, wraps each event in an NDKEvent class instance',
			'dedups by event id across relays (duplicates fire event:dup, counted but not delivered)',
			'EOSE is aggregate-only in NDK 3.x (emitted once all relays EOSEd)',
			'DEFAULT OVERRIDDEN: schnorr signature verification ON by default (skipVerification: true set)',
			'DEFAULT OVERRIDDEN: kind-schema validation ON by default (skipValidation: true set)',
			'groupable subscription batching disabled (groupable: false) so the REQ is sent immediately',
			'no persistence (no cache adapter configured)'
		],
		async setup(relays) {
			ndk = new NDK({ explicitRelayUrls: [...relays] });
		},
		run(relays, n, subId, onEvent, timeoutMs): Promise<MultiRunResult> {
			return new Promise((resolve) => {
				const tr = new RelayTracker(relays.length);
				const notes: string[] = ['cross-relay dedup: YES (subscription-level id set, event:dup for repeats)'];
				let dupCount = 0;
				let done = false;
				let sub: ReturnType<NDK['subscribe']> | null = null;
				const finish = (reason?: string) => {
					if (done) return;
					done = true;
					clearTimeout(timer);
					try {
						sub?.stop();
					} catch {
						/* already stopped */
					}
					if (dupCount > 0) notes.push(`${dupCount} duplicate events suppressed by NDK dedup`);
					if (reason) notes.push(reason);
					resolve(tr.result(notes));
				};
				const timer = setTimeout(
					() => finish(`TIMEOUT after ${timeoutMs}ms (partial results)`),
					timeoutMs
				);

				// Per-relay connect tracking: pool relays were created in setup().
				for (const relay of ndk!.pool.relays.values()) {
					(relay as any).on?.('connect', () => tr.markOpen(relay.url));
				}
				// Fire-and-forget: the subscription below waits for connections, so
				// connect time stays inside the measured window.
				ndk!.connect(10000).catch(() => {});

				sub = ndk!.subscribe(
					{ kinds: [1], limit: n },
					{ subId, skipVerification: true, skipValidation: true, closeOnEose: true, groupable: false }
				);
				sub.on('event', (ev: any) => {
					tr.markEvent();
					onEvent(ev.id);
				});
				sub.on('event:dup', () => {
					dupCount++;
				});
				sub.on('eose', () => {
					// NDK 3.x emits 'eose' once, after every relay in the sub's
					// relay set has EOSEd — aggregate-only, like nostr-tools.
					for (const url of relays) tr.markEose(url);
					notes.push('EOSE reported aggregate-only (NDK emits eose once all relays EOSEd)');
					setTimeout(() => finish(), 300);
				});
			});
		},
		async teardown() {
			try {
				for (const relay of ndk?.pool?.relays?.values?.() ?? []) {
					(relay as any).disconnect?.();
				}
			} catch {
				/* best effort */
			}
			ndk = null;
		}
	};
}
