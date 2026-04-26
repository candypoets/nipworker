import { createNostrManager, setManager } from '../../src/index';
import { useSubscription } from '../../src/hooks';
import { WorkerMessage, MessageType, ParsedEvent } from '../../src/generated/nostr/fb';

const RELAY = 'wss://nos.lol';

interface TestResults {
	success: boolean;
	errors: string[];
	eventsReceived: number;
	eoceReceived: boolean;
	kinds: number[];
	messageTypes: string[];
}

(window as any).__testResults = null;

function typeName(msg: WorkerMessage): string {
	const t = msg.type();
	switch (t) {
		case MessageType.ParsedNostrEvent: return 'ParsedNostrEvent';
		case MessageType.Eoce: return 'Eoce';
		case MessageType.ConnectionStatus: return 'ConnectionStatus';
		case MessageType.NostrEvent: return 'NostrEvent';
		default: return `Type(${t})`;
	}
}

async function runTest(): Promise<TestResults> {
	const R: TestResults = {
		success: false,
		errors: [],
		eventsReceived: 0,
		eoceReceived: false,
		kinds: [],
		messageTypes: [],
	};

	const logEl = document.getElementById('log')!;
	const statusEl = document.getElementById('status')!;
	const log = (s: string) => { logEl.textContent += s + '\n'; console.log(s); };

	try {
		statusEl.textContent = 'Booting engine...';
		const manager = createNostrManager({ engine: true });
		setManager(manager);
		log('✓ EngineManager ready');

		// Wait for worker init
		await new Promise((r) => setTimeout(r, 1500));

		statusEl.textContent = 'Subscribing...';
		log('\n=== useSubscription Test ===');

		await new Promise<void>((resolve) => {
			const unsub = useSubscription(
				'sub_wasm_test',
				[{ kinds: [1], limit: 10, relays: [RELAY] }],
				(msg) => {
					const tn = typeName(msg);
					R.messageTypes.push(tn);
					log(`[msg] ${tn}`);

					if (msg.type() === MessageType.ParsedNostrEvent) {
						R.eventsReceived++;
						const parsed = msg.content(new ParsedEvent());
						if (parsed) {
							R.kinds.push(parsed.kind());
						}
					} else if (msg.type() === MessageType.Eoce) {
						R.eoceReceived = true;
					}
				},
				{ closeOnEose: true, bytesPerEvent: 8192 }
			);

			// Wait full duration — don't resolve early on EOCE,
			// as events can arrive after EOCE in some engine paths
			setTimeout(() => {
				unsub();
				resolve();
			}, 20000);
		});

		log(`Events received: ${R.eventsReceived}`);
		log(`EOCE received: ${R.eoceReceived}`);
		log(`Kinds: [${R.kinds.join(', ')}]`);

		if (R.eventsReceived === 0) {
			R.errors.push('No events received from subscription');
		}
		if (!R.eoceReceived) {
			R.errors.push('EOCE not received');
		}

		R.success = R.errors.length === 0 && R.eventsReceived > 0 && R.eoceReceived;
		statusEl.textContent = R.success ? '✅ TEST PASSED' : '❌ TEST FAILED';
	} catch (e: any) {
		R.errors.push(String(e.message || e));
		R.success = false;
		statusEl.textContent = '❌ EXCEPTION';
	}

	(window as any).__testResults = R;
	return R;
}

runTest();
