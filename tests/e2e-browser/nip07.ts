import { createNostrManager, setManager } from '../../src/index';
import type { EventTemplate, NostrEvent } from 'nostr-tools';

const MOCK_PUBKEY = 'a1b2c3d4e5f6789012345678901234567890123456789012345678901234567890';
const MOCK_SIG = 'abcd1234efgh5678ijkl9012mnop3456qrst7890uvwx1234yzab5678cdef9012abcd1234efgh5678ijkl9012mnop3456qrst7890uvwx1234yzab5678cdef9012';
const MOCK_EVENT_ID = 'deadbeef1234567890abcdef1234567890abcdef1234567890abcdef12345678';

interface TestResults {
	success: boolean;
	errors: string[];
	pubkeySet: boolean;
	activePubkey: string | null;
	signEventCalled: boolean;
	signEventPayload: any;
	signedEvent: NostrEvent | null;
	getPublicKeyCalled: boolean;
}

(window as any).__testResults = null;
(window as any).__signEventCalls = [];
(window as any).__getPublicKeyCalls = 0;

// Mock window.nostr BEFORE booting the engine
(window as any).nostr = {
	getPublicKey: async (): Promise<string> => {
		(window as any).__getPublicKeyCalls++;
		return MOCK_PUBKEY;
	},
	signEvent: async (payload: any): Promise<NostrEvent> => {
		const template = typeof payload === 'string' ? JSON.parse(payload) : payload;
		(window as any).__signEventCalls.push(template);

		return {
			id: MOCK_EVENT_ID,
			pubkey: MOCK_PUBKEY,
			created_at: template.created_at,
			kind: template.kind,
			tags: template.tags,
			content: template.content,
			sig: MOCK_SIG
		};
	}
};

async function runTest(): Promise<TestResults> {
	const R: TestResults = {
		success: false,
		errors: [],
		pubkeySet: false,
		activePubkey: null,
		signEventCalled: false,
		signEventPayload: null,
		signedEvent: null,
		getPublicKeyCalled: false
	};

	const logEl = document.getElementById('log')!;
	const statusEl = document.getElementById('status')!;
	const log = (s: string) => { logEl.textContent += s + '\n'; console.log(s); };

	try {
		// Clear any previous session to avoid auto-restore
		localStorage.removeItem('nostr_active_pubkey');
		localStorage.removeItem('nostr_signer_accounts');

		statusEl.textContent = 'Booting engine...';
		const manager = createNostrManager({ engine: true });
		setManager(manager);

		log('✓ EngineManager created');

		// Wait for initial auth (pubkey null from restoreSession)
		await new Promise<void>((resolve) => {
			const handler = ((evt: CustomEvent) => {
				if (evt.detail.pubkey === null) {
					manager.removeEventListener('auth', handler as EventListener);
					resolve();
				}
			}) as EventListener;
			manager.addEventListener('auth', handler);
			setTimeout(() => {
				manager.removeEventListener('auth', handler as EventListener);
				resolve();
			}, 2000);
		});

		// ---- Test 1: setNip07 triggers getPublicKey ----
		statusEl.textContent = 'Setting NIP-07 signer...';
		log('→ Calling setNip07()');

		await new Promise<void>((resolve, reject) => {
			const timeout = setTimeout(() => reject(new Error('Timeout waiting for NIP-07 auth')), 15000);
			const handler = ((evt: CustomEvent) => {
				clearTimeout(timeout);
				R.pubkeySet = evt.detail.hasSigner;
				R.activePubkey = evt.detail.pubkey;
				manager.removeEventListener('auth', handler as EventListener);
				resolve();
			}) as EventListener;
			manager.addEventListener('auth', handler);
			manager.setNip07();
		});

		R.getPublicKeyCalled = (window as any).__getPublicKeyCalls > 0;
		log(`✓ NIP-07 set: pubkey=${R.activePubkey?.slice(0, 16)}... hasSigner=${R.pubkeySet}`);
		log(`✓ getPublicKey called: ${R.getPublicKeyCalled}`);

		// ---- Test 2: signEvent flows through the full pipeline ----
		statusEl.textContent = 'Testing signEvent...';
		log('→ Calling signEvent()');

		const template: EventTemplate = {
			kind: 1,
			created_at: Math.floor(Date.now() / 1000),
			tags: [['t', 'nip07-e2e']],
			content: 'Hello from NIP-07 E2E test'
		};

		const signedEvent = await new Promise<NostrEvent>((resolve, reject) => {
			const timeout = setTimeout(() => reject(new Error('Timeout waiting for signed event')), 15000);
			manager.signEvent(template, (event) => {
				clearTimeout(timeout);
				resolve(event);
			});
		});

		R.signedEvent = signedEvent;
		R.signEventCalled = (window as any).__signEventCalls.length > 0;
		R.signEventPayload = (window as any).__signEventCalls[0] || null;

		log(`✓ signEvent returned: id=${signedEvent.id.slice(0, 16)}...`);
		log(`✓ window.nostr.signEvent called: ${R.signEventCalled}`);

		// ---- Validation ----
		if (R.activePubkey !== MOCK_PUBKEY) {
			R.errors.push(`Expected pubkey ${MOCK_PUBKEY}, got ${R.activePubkey}`);
		}
		if (!R.getPublicKeyCalled) {
			R.errors.push('window.nostr.getPublicKey was not called');
		}
		if (!R.signEventCalled) {
			R.errors.push('window.nostr.signEvent was not called');
		}
		if (signedEvent.pubkey !== MOCK_PUBKEY) {
			R.errors.push(`Expected signed event pubkey ${MOCK_PUBKEY}, got ${signedEvent.pubkey}`);
		}
		if (signedEvent.kind !== template.kind) {
			R.errors.push(`Expected kind ${template.kind}, got ${signedEvent.kind}`);
		}
		if (signedEvent.content !== template.content) {
			R.errors.push(`Expected content "${template.content}", got "${signedEvent.content}"`);
		}
		if (signedEvent.sig !== MOCK_SIG) {
			R.errors.push(`Expected sig ${MOCK_SIG}, got ${signedEvent.sig}`);
		}
		if (R.signEventPayload) {
			const payload = typeof R.signEventPayload === 'string'
				? JSON.parse(R.signEventPayload)
				: R.signEventPayload;
			if (payload.kind !== template.kind) {
				R.errors.push(`signEvent payload kind mismatch: ${payload.kind}`);
			}
			if (payload.content !== template.content) {
				R.errors.push(`signEvent payload content mismatch: ${payload.content}`);
			}
		}

		R.success = R.errors.length === 0;
		statusEl.textContent = R.success ? '✅ TEST PASSED' : '❌ TEST FAILED';
		if (R.errors.length > 0) {
			R.errors.forEach(e => log(`✗ ${e}`));
		}

	} catch (e: any) {
		R.errors.push(String(e.message || e));
		R.success = false;
		statusEl.textContent = '❌ EXCEPTION';
	}

	(window as any).__testResults = R;
	return R;
}

runTest();
