import { createNostrManager, setManager } from '../../src/index';
import { verifyEvent } from 'nostr-tools';
import type { EventTemplate, NostrEvent } from 'nostr-tools';

// Must match the fixed keypair in mock-signer-relay.mjs (secret key 'aa'.repeat(32)).
const SIGNER_PUBKEY = '6a04ab98d9e4774ad806e302dddeb63bea16b5cb5f223ee77478e861bb583eb3';
const RELAY_URL = 'ws://localhost:7746';
const BUNKER_URL = `bunker://${SIGNER_PUBKEY}?relay=${encodeURIComponent(RELAY_URL)}`;

interface TestResults {
	success: boolean;
	errors: string[];
	pubkeySet: boolean;
	activePubkey: string | null;
	signedEvent: NostrEvent | null;
	signatureValid: boolean;
}

(window as any).__testResults = null;

async function runTest(): Promise<TestResults> {
	const R: TestResults = {
		success: false,
		errors: [],
		pubkeySet: false,
		activePubkey: null,
		signedEvent: null,
		signatureValid: false
	};

	const logEl = document.getElementById('log')!;
	const statusEl = document.getElementById('status')!;
	const log = (s: string) => { logEl.textContent += s + '\n'; console.log(s); };

	try {
		// Clear any previous session to avoid auto-restore
		localStorage.removeItem('nostr_active_pubkey');
		localStorage.removeItem('nostr_signer_accounts');

		statusEl.textContent = 'Booting workers...';
		const manager = createNostrManager({ logLevel: 'info' } as any);
		setManager(manager);

		log('✓ NostrManager created');

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

		// ---- Test 1: setNip46Bunker connects and resolves the user pubkey ----
		statusEl.textContent = 'Setting NIP-46 bunker signer...';
		log(`→ Calling setNip46Bunker(${BUNKER_URL})`);

		await new Promise<void>((resolve, reject) => {
			const timeout = setTimeout(() => reject(new Error('Timeout waiting for NIP-46 auth')), 25000);
			const handler = ((evt: CustomEvent) => {
				if (evt.detail.pubkey === null) return; // ignore the initial restore event
				clearTimeout(timeout);
				R.pubkeySet = evt.detail.hasSigner;
				R.activePubkey = evt.detail.pubkey;
				manager.removeEventListener('auth', handler as EventListener);
				resolve();
			}) as EventListener;
			manager.addEventListener('auth', handler);
			manager.setNip46Bunker(BUNKER_URL);
		});

		log(`✓ NIP-46 set: pubkey=${R.activePubkey?.slice(0, 16)}... hasSigner=${R.pubkeySet}`);

		// ---- Test 2: sign_event RPC round-trip through the mock signer ----
		statusEl.textContent = 'Testing signEvent...';
		log('→ Calling signEvent()');

		const template: EventTemplate = {
			kind: 1,
			created_at: Math.floor(Date.now() / 1000),
			tags: [['t', 'nip46-e2e']],
			content: 'Hello from NIP-46 E2E test'
		};

		const signedEvent = await new Promise<NostrEvent>((resolve, reject) => {
			const timeout = setTimeout(() => reject(new Error('Timeout waiting for signed event')), 25000);
			manager.signEvent(template, (event) => {
				clearTimeout(timeout);
				resolve(event);
			});
		});

		R.signedEvent = signedEvent;
		R.signatureValid = verifyEvent(signedEvent);

		log(`✓ signEvent returned: id=${signedEvent.id.slice(0, 16)}... sigValid=${R.signatureValid}`);

		// ---- Validation ----
		if (R.activePubkey !== SIGNER_PUBKEY) {
			R.errors.push(`Expected pubkey ${SIGNER_PUBKEY}, got ${R.activePubkey}`);
		}
		if (!R.pubkeySet) {
			R.errors.push('auth event did not report hasSigner');
		}
		if (signedEvent.pubkey !== SIGNER_PUBKEY) {
			R.errors.push(`Expected signed event pubkey ${SIGNER_PUBKEY}, got ${signedEvent.pubkey}`);
		}
		if (signedEvent.kind !== template.kind) {
			R.errors.push(`Expected kind ${template.kind}, got ${signedEvent.kind}`);
		}
		if (signedEvent.content !== template.content) {
			R.errors.push(`Expected content "${template.content}", got "${signedEvent.content}"`);
		}
		if (!R.signatureValid) {
			R.errors.push('Signed event failed verifyEvent()');
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
