import { MessageType } from '../../src/generated/nostr/fb';

// Loads the BUILT bundle (/dist) rather than src: worker URLs in src point at
// .js files that don't exist under the vite dev server, while dist is
// self-contained. This exercises the exact code that ships.
interface Results {
	ok: boolean;
	networkEvents: number;
	eoce: boolean;
	cachedEvents: number;
	errors: string[];
}

(window as any).__testResults = null;

const RELAY = 'wss://nos.lol';

async function main(): Promise<void> {
	const R: Results = { ok: false, networkEvents: 0, eoce: false, cachedEvents: 0, errors: [] };
	const logEl = document.getElementById('log')!;
	const log = (s: string) => {
		logEl.textContent += s + '\n';
		console.log(s);
	};

	try {
		const { createNostrManager, setManager } = await import('/dist/index.js');
		const { useSubscription } = await import('/dist/hooks.js');
		const manager = createNostrManager();
		setManager(manager);
		log('manager ready');

		// Give the cache worker a moment to initialize (and, on insecure
		// origins, to fail OPFS hydration and fall back to memory).
		await new Promise((r) => setTimeout(r, 1500));

		// Network subscription: proves the pipeline works with an empty cache.
		await new Promise<void>((resolve) => {
			const unsub = useSubscription(
				'sub_net',
				[{ kinds: [1], limit: 5, relays: [RELAY] }],
				(msg) => {
					if (msg.type() === MessageType.ParsedNostrEvent) R.networkEvents++;
				},
				{ closeOnEose: true, bytesPerEvent: 8192 }
			);
			setTimeout(() => {
				unsub();
				resolve();
			}, 15000);
		});
		log(`network events: ${R.networkEvents}`);

		// Cache-only query: must terminate with EOCE even with nothing
		// persisted (in-memory cache may hold sub_net's events).
		await new Promise<void>((resolve) => {
			const unsub = useSubscription(
				'sub_cache',
				[{ kinds: [1], limit: 5, relays: [RELAY] }],
				(msg) => {
					if (msg.type() === MessageType.ParsedNostrEvent) R.cachedEvents++;
					if (msg.type() === MessageType.Eoce) {
						R.eoce = true;
						unsub();
						resolve();
					}
				},
				{ closeOnEose: true, bytesPerEvent: 8192, cacheOnly: true }
			);
			setTimeout(() => {
				unsub();
				resolve();
			}, 5000);
		});
		log(`cache-only: eoce=${R.eoce} cachedEvents=${R.cachedEvents}`);

		R.ok = R.networkEvents > 0 && R.eoce;
	} catch (e: any) {
		R.errors.push(String(e?.message || e));
	}

	(window as any).__testResults = R;
}

main();
