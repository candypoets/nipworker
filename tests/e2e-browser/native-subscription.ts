import { createNostrManager, setManager } from '../../src/native';
import { useSubscription } from '../../src/hooks';
import { isParsedEvent, isEoce } from '../../src/lib/NarrowTypes';
import type { NostrManagerLike } from '../../src/manager';
import type { RequestObject } from '../../src/types';

import './native-subscription-mock';

const statusEl = document.getElementById('status') as HTMLParagraphElement;
const eventsEl = document.getElementById('events') as HTMLUListElement;
const eoseEl = document.getElementById('eose') as HTMLParagraphElement;

async function runTest() {
	const manager: NostrManagerLike = createNostrManager({
		relays: ['ws://localhost:8180'],
	});

	setManager(manager);

	// Wait for auth event after setting signer
	const authPromise = new Promise<string>((resolve) => {
		const handler = ((evt: CustomEvent) => {
			if (evt.detail.pubkey) {
				manager.removeEventListener('auth', handler as EventListener);
				resolve(evt.detail.pubkey);
			}
		}) as EventListener;
		manager.addEventListener('auth', handler);
		// Timeout fallback
		setTimeout(() => {
			manager.removeEventListener('auth', handler as EventListener);
			resolve('');
		}, 5000);
	});

	manager.setSigner('privkey', '0000000000000000000000000000000000000000000000000000000000000001');
	const pubkey = await authPromise;
	statusEl.textContent = 'Pubkey: ' + pubkey;

	const requests: RequestObject[] = [
		{
			relays: ['ws://localhost:8180'],
			kinds: [1],
			limit: 10,
		},
	];

	const events: any[] = [];
	let eoseReceived = false;

	const unsubscribe = useSubscription(
		'native-test-sub',
		requests,
		(msg) => {
			const parsed = isParsedEvent(msg);
			if (parsed) {
				const event = {
					id: parsed.id(),
					pubkey: parsed.pubkey(),
					kind: parsed.kind(),
					createdAt: parsed.createdAt(),
				};
				const li = document.createElement('li');
				li.className = 'event-item';
				li.textContent = `kind=${event.kind}, id=${event.id}, pubkey=${event.pubkey}`;
				eventsEl.appendChild(li);
				events.push(event);
			}

			const eoce = isEoce(msg);
			if (eoce) {
				eoseReceived = true;
				eoseEl.textContent = 'EOSE received, eventCount=' + events.length;
				(window as any).__testResult = { events, eose: true, pubkey };
			}
		},
		{ closeOnEose: true }
	);

	// Safety timeout to capture result if EOSE never arrives
	setTimeout(() => {
		if (!(window as any).__testResult) {
			(window as any).__testResult = { events, eose: eoseReceived, pubkey, timeout: true };
		}
	}, 3000);
}

runTest().catch((err) => {
	statusEl.textContent = 'Fatal error: ' + String(err);
	(window as any).__testResult = { events: [], eose: false, error: String(err), fatal: true };
});
