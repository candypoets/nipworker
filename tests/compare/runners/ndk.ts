import NDK from '@nostr-dev-kit/ndk';

import type { ContenderRunner, RunResult } from './types';

// NDK 3.x. Defaults: signature verification ON (async verifier), kind-schema
// validation ON, dedup ON (event:dup for repeats), groupable subscriptions.
// Verification/validation are overridden because the mock relay serves
// synthetic unsigned events.
export function createNdkRunner(): ContenderRunner {
	let ndk: NDK | null = null;
	const dups: number[] = [];

	return {
		name: 'ndk',
		perEventWork: [
			'JSON.parse per EVENT frame, wraps each event in an NDKEvent class instance',
			'dedups by event id across relays (duplicate deliveries counted separately)',
			'DEFAULT OVERRIDDEN: schnorr signature verification ON by default (skipVerification: true set)',
			'DEFAULT OVERRIDDEN: kind-schema validation ON by default (skipValidation: true set)',
			'groupable subscription batching left at default (up to 100ms "at-most" delay on first sub)',
			'no persistence (no cache adapter configured)'
		],
		async setup(relay: string) {
			ndk = new NDK({ explicitRelayUrls: [relay] });
			// Fire-and-forget: subscriptions issued below wait for the connection,
			// so connection setup is inside the measured window like the others.
			ndk.connect(5000).catch(() => {});
		},
		run(relay: string, n: number, subId: string, onEvent: () => void): Promise<RunResult> {
			return new Promise((resolve) => {
				let received = 0;
				let dupCount = 0;
				let done = false;
				const finish = () => {
					if (done) return;
					done = true;
					try {
						sub.stop();
					} catch {
						/* already stopped */
					}
					dups.push(dupCount);
					resolve({
						received,
						rawCount: received + dupCount,
						notes: dupCount > 0 ? [`${dupCount} duplicate events suppressed by NDK dedup`] : []
					});
				};
				const sub = ndk!.subscribe(
					{ kinds: [1], limit: n },
					{
						subId,
						skipVerification: true,
						skipValidation: true,
						closeOnEose: true
					}
				);
				sub.on('event', () => {
					received++;
					onEvent();
					if (received >= n) setTimeout(finish, 0);
				});
				sub.on('event:dup', () => {
					dupCount++;
				});
				sub.on('eose', () => {
					setTimeout(finish, 500);
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
