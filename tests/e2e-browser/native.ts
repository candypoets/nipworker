import { createNostrManager, setManager } from '../../src/native';
import type { NostrEvent } from 'nostr-tools';

const TEST_PRIVKEY = '0000000000000000000000000000000000000000000000000000000000000001';
const EXPECTED_PUBKEY = '79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798';

interface TestResults {
	success: boolean;
	errors: string[];

	// Test 1: Backend creation
	backendCreated: boolean;

	// Test 2: Privkey signer
	signerSet: boolean;
	signerPubkey: string | null;

	// Test 3: getPublicKey
	getPublicKeyWorks: boolean;
	getPublicKeyResult: string | null;

	// Test 4: signEvent
	signEventWorks: boolean;
	signedEvent: NostrEvent | null;

	// Test 5: Session persistence
	sessionPersisted: boolean;

	// Test 6: Logout
	logoutWorks: boolean;
	activePubkeyAfterLogout: string | null;
}

(window as any).__testResults = null;

async function runTest(): Promise<TestResults> {
	const R: TestResults = {
		success: false,
		errors: [],
		backendCreated: false,
		signerSet: false,
		signerPubkey: null,
		getPublicKeyWorks: false,
		getPublicKeyResult: null,
		signEventWorks: false,
		signedEvent: null,
		sessionPersisted: false,
		logoutWorks: false,
		activePubkeyAfterLogout: null,
	};

	const logEl = document.getElementById('log')!;
	const statusEl = document.getElementById('status')!;
	const log = (s: string) => { logEl.textContent += s + '\n'; console.log(s); };

	try {
		// Clear any previous session
		localStorage.removeItem('nostr_active_pubkey');
		localStorage.removeItem('nostr_signer_accounts');

		// ---- Test 1: Create backend ----
		statusEl.textContent = 'Test 1: Creating NativeBackend...';
		log('\n=== Test 1: NativeBackend Creation ===');

		const manager = createNostrManager();
		setManager(manager);
		R.backendCreated = true;
		log('✓ NativeBackend created');

		// ---- Test 2: Privkey signer ----
		statusEl.textContent = 'Test 2: Privkey signer...';
		log('\n=== Test 2: Privkey Signer ===');

		const authPromise = new Promise<{ pubkey: string; hasSigner: boolean }>((resolve, reject) => {
			const timeout = setTimeout(() => reject(new Error('Timeout waiting for auth event')), 10000);
			const handler = ((evt: CustomEvent) => {
				// Ignore the initial null-auth from restoreSession()
				if (!evt.detail.pubkey) return;
				clearTimeout(timeout);
				manager.removeEventListener('auth', handler as EventListener);
				resolve({ pubkey: evt.detail.pubkey, hasSigner: evt.detail.hasSigner });
			}) as EventListener;
			manager.addEventListener('auth', handler);
		});

		manager.setSigner('privkey', TEST_PRIVKEY);
		const authResult = await authPromise;

		R.signerSet = authResult.hasSigner;
		R.signerPubkey = authResult.pubkey;

		if (!R.signerSet) {
			R.errors.push('setSigner: auth event did not report hasSigner=true');
		} else {
			log(`✓ Signer set: pubkey=${R.signerPubkey?.slice(0, 16)}...`);
		}

		// ---- Test 3: getPublicKey ----
		statusEl.textContent = 'Test 3: getPublicKey...';
		log('\n=== Test 3: getPublicKey ===');

		const pkPromise = new Promise<{ pubkey: string; hasSigner: boolean }>((resolve, reject) => {
			const timeout = setTimeout(() => reject(new Error('Timeout waiting for getPublicKey response')), 10000);
			const handler = ((evt: CustomEvent) => {
				if (!evt.detail.pubkey) return;
				clearTimeout(timeout);
				manager.removeEventListener('auth', handler as EventListener);
				resolve({ pubkey: evt.detail.pubkey, hasSigner: evt.detail.hasSigner });
			}) as EventListener;
			manager.addEventListener('auth', handler);
		});

		manager.getPublicKey();
		const pkResult = await pkPromise;

		R.getPublicKeyResult = pkResult.pubkey;
		R.getPublicKeyWorks = R.getPublicKeyResult === EXPECTED_PUBKEY;

		if (!R.getPublicKeyWorks) {
			R.errors.push(`getPublicKey: expected ${EXPECTED_PUBKEY}, got ${R.getPublicKeyResult}`);
		} else {
			log('✓ getPublicKey returns expected pubkey');
		}

		// ---- Test 4: signEvent ----
		statusEl.textContent = 'Test 4: signEvent...';
		log('\n=== Test 4: signEvent ===');

		const template = {
			kind: 1,
			created_at: Math.floor(Date.now() / 1000),
			tags: [['t', 'native-e2e']],
			content: 'Hello from NativeBackend E2E test',
		};

		const signedEvent = await new Promise<NostrEvent>((resolve, reject) => {
			const timeout = setTimeout(() => reject(new Error('Timeout waiting for signed event')), 10000);
			manager.signEvent(template, (event) => {
				clearTimeout(timeout);
				resolve(event);
			});
		});

		R.signedEvent = signedEvent;
		R.signEventWorks = !!signedEvent.pubkey && signedEvent.pubkey === EXPECTED_PUBKEY;

		if (!R.signEventWorks) {
			R.errors.push(`signEvent: pubkey mismatch. expected=${EXPECTED_PUBKEY}, got=${signedEvent.pubkey}`);
		} else {
			log(`✓ signEvent returned event with matching pubkey`);
			log(`  id=${signedEvent.id.slice(0, 16)}... sig=${signedEvent.sig.slice(0, 16)}...`);
		}

		// Validate event fields
		if (signedEvent.kind !== template.kind) {
			R.errors.push(`signEvent: kind mismatch`);
		}
		if (signedEvent.content !== template.content) {
			R.errors.push(`signEvent: content mismatch`);
		}

		// ---- Test 5: Session persistence ----
		statusEl.textContent = 'Test 5: Session persistence...';
		log('\n=== Test 5: Session Persistence ===');

		const storedPubkey = localStorage.getItem('nostr_active_pubkey');
		const storedAccounts = localStorage.getItem('nostr_signer_accounts');
		R.sessionPersisted = storedPubkey === EXPECTED_PUBKEY && !!storedAccounts;

		if (!R.sessionPersisted) {
			R.errors.push(`session persistence: storedPubkey=${storedPubkey}`);
		} else {
			log('✓ Session persisted to localStorage');
		}

		// ---- Test 6: Logout ----
		statusEl.textContent = 'Test 6: Logout...';
		log('\n=== Test 6: Logout ===');

		manager.logout();
		R.activePubkeyAfterLogout = manager.getActivePubkey();
		R.logoutWorks = R.activePubkeyAfterLogout === null;

		if (!R.logoutWorks) {
			R.errors.push(`logout: expected null, got ${R.activePubkeyAfterLogout}`);
		} else {
			log('✓ Logout cleared active pubkey');
		}

		// Overall success
		R.success = R.errors.length === 0 && R.backendCreated && R.signerSet && R.getPublicKeyWorks && R.signEventWorks && R.sessionPersisted && R.logoutWorks;
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
