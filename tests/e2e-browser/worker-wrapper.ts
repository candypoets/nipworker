import { createNostrManager, setManager } from '../../src/index';
import { useSubscription } from '../../src/hooks';
import type { EventTemplate, NostrEvent } from 'nostr-tools';
import { WorkerMessage, MessageType, ParsedEvent } from '../../src/generated/nostr/fb';

const RELAY = 'wss://nos.lol';
const TEST_PRIVKEY = '0000000000000000000000000000000000000000000000000000000000000001';

interface MsgRecord {
  type: string;
  ts: number;
  event?: ParsedEvent;
}

interface TestResults {
  success: boolean;
  errors: string[];

  // Test 1: Basic subscribe
  subscribeWorks: boolean;
  sub1Events: number;
  sub1Eoce: boolean;

  // Test 2: Privkey signer
  signerSet: boolean;
  signerPubkey: string | null;
  getPublicKeyWorks: boolean;
  getPublicKeyResult: string | null;

  // Test 3: signEvent
  signEventWorks: boolean;
  signedEvent: NostrEvent | null;

  // Test 4: Session persistence
  sessionPersisted: boolean;
  sessionRestoredAfterReload: boolean;
  restoredPubkey: string | null;
}

(window as any).__testResults = null;

function typeName(msg: WorkerMessage): string {
  const t = msg.type();
  switch (t) {
    case MessageType.ParsedNostrEvent: return 'ParsedNostrEvent';
    case MessageType.Eoce: return 'Eoce';
    case MessageType.ConnectionStatus: return 'ConnectionStatus';
    default: return `Type(${t})`;
  }
}

function extractEvent(msg: WorkerMessage): ParsedEvent | null {
  if (msg.type() === MessageType.ParsedNostrEvent) {
    return msg.content(new ParsedEvent()) as ParsedEvent;
  }
  return null;
}

async function runTest(): Promise<TestResults> {
  const R: TestResults = {
    success: false,
    errors: [],
    subscribeWorks: false,
    sub1Events: 0,
    sub1Eoce: false,
    signerSet: false,
    signerPubkey: null,
    getPublicKeyWorks: false,
    getPublicKeyResult: null,
    signEventWorks: false,
    signedEvent: null,
    sessionPersisted: false,
    sessionRestoredAfterReload: false,
    restoredPubkey: null
  };

  const logEl = document.getElementById('log')!;
  const statusEl = document.getElementById('status')!;
  const log = (s: string) => { logEl.textContent += s + '\n'; console.log(s); };

  const urlParams = new URLSearchParams(window.location.search);
  const isReload = urlParams.get('reload') === '1';

  try {
    // Clear any previous session for clean test (only on initial load)
    if (!isReload) {
      localStorage.removeItem('nostr_active_pubkey');
      localStorage.removeItem('nostr_signer_accounts');
    }

    statusEl.textContent = 'Booting 4-worker WASM...';
    const manager = createNostrManager();
    setManager(manager);

    log('✓ NostrManager (4-worker) created');

    // In phase 2 (after reload), attach auth listener BEFORE workers init
    // because restoreSession() fires as a microtask in the constructor
    let restorePromise: Promise<{ pubkey: string; hasSigner: boolean }> | null = null;
    let expectedPubkey: string | null = null;
    if (isReload) {
      log('\n=== Phase 2: After Reload ===');

      expectedPubkey = localStorage.getItem('nostr_active_pubkey');
      const storedAccounts = localStorage.getItem('nostr_signer_accounts');
      log(`Phase 2: expectedPubkey=${expectedPubkey?.slice(0, 16)}..., accounts=${storedAccounts?.slice(0, 100)}...`);

      log('Phase 2: attaching auth listener...');
      restorePromise = new Promise<{ pubkey: string; hasSigner: boolean }>((resolve, reject) => {
        const timeout = setTimeout(() => reject(new Error('Timeout waiting for session restore')), 15000);
        const handler = ((evt: CustomEvent) => {
          if (!evt.detail.pubkey) return;
          clearTimeout(timeout);
          manager.removeEventListener('auth', handler as EventListener);
          resolve({ pubkey: evt.detail.pubkey, hasSigner: evt.detail.hasSigner });
        }) as EventListener;
        manager.addEventListener('auth', handler);
      });
    }

    // Wait a bit for workers to init
    await new Promise(r => setTimeout(r, 1500));

    if (isReload && restorePromise) {

      const restoreResult = await restorePromise;
      R.restoredPubkey = restoreResult.pubkey;
      R.sessionRestoredAfterReload = R.restoredPubkey === expectedPubkey;
      R.signerSet = restoreResult.hasSigner;
      R.signerPubkey = restoreResult.pubkey;

      if (!R.sessionRestoredAfterReload) {
        R.errors.push(`session restore: expected=${expectedPubkey}, got=${R.restoredPubkey}`);
      } else {
        log(`✓ Session restored after reload: pubkey=${R.restoredPubkey?.slice(0, 16)}...`);
      }

      // Also verify getPublicKey works on restored session
      const pkPromise = new Promise<{ pubkey: string; hasSigner: boolean }>((resolve, reject) => {
        const timeout = setTimeout(() => reject(new Error('Timeout waiting for getPublicKey response')), 10000);
        const handler = ((evt: CustomEvent) => {
          clearTimeout(timeout);
          manager.removeEventListener('auth', handler as EventListener);
          resolve({ pubkey: evt.detail.pubkey, hasSigner: evt.detail.hasSigner });
        }) as EventListener;
        manager.addEventListener('auth', handler);
      });

      manager.getPublicKey();
      const pkResult = await pkPromise;

      R.getPublicKeyResult = pkResult.pubkey;
      R.getPublicKeyWorks = R.getPublicKeyResult === R.signerPubkey;

      if (!R.getPublicKeyWorks) {
        R.errors.push(`getPublicKey after restore: expected ${R.signerPubkey}, got ${R.getPublicKeyResult}`);
      } else {
        log('✓ getPublicKey returns consistent pubkey after restore');
      }
    } else {
      // ---- Phase 1: Run all tests ----

      // ---- Test 1: Basic subscribe ----
      statusEl.textContent = 'Test 1: Basic subscribe...';
      log('\n=== Test 1: Basic Subscribe ===');

      const sub1Records: MsgRecord[] = [];
      await new Promise<void>((resolve) => {
        const unsub = useSubscription('sub1', [{ kinds: [1], limit: 5, relays: [RELAY] }], (msg) => {
          const record: MsgRecord = { type: typeName(msg), ts: Date.now() };
          const event = extractEvent(msg);
          if (event) record.event = event;
          sub1Records.push(record);
        }, { closeOnEose: true, bytesPerEvent: 8192 });
        setTimeout(() => { unsub(); resolve(); }, 30000);
      });

      R.sub1Events = sub1Records.filter(m => m.type === 'ParsedNostrEvent').length;
      R.sub1Eoce = sub1Records.some(m => m.type === 'Eoce');
      R.subscribeWorks = R.sub1Events > 0;

      log(`sub1: ${sub1Records.length} messages, ${R.sub1Events} events, EOCE=${R.sub1Eoce}`);
      if (!R.subscribeWorks) {
        R.errors.push(`subscribe: expected events>0, got events=${R.sub1Events}`);
      } else {
        log('✓ Subscribe works');
      }

      // ---- Test 2: Privkey signer + getPublicKey ----
      statusEl.textContent = 'Test 2: Privkey signer...';
      log('\n=== Test 2: Privkey Signer + getPublicKey ===');

      const authPromise = new Promise<{ pubkey: string; hasSigner: boolean }>((resolve, reject) => {
        const timeout = setTimeout(() => reject(new Error('Timeout waiting for auth event')), 10000);
        const handler = ((evt: CustomEvent) => {
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

      // Now test getPublicKey
      const pkPromise = new Promise<{ pubkey: string; hasSigner: boolean }>((resolve, reject) => {
        const timeout = setTimeout(() => reject(new Error('Timeout waiting for getPublicKey response')), 10000);
        const handler = ((evt: CustomEvent) => {
          clearTimeout(timeout);
          manager.removeEventListener('auth', handler as EventListener);
          resolve({ pubkey: evt.detail.pubkey, hasSigner: evt.detail.hasSigner });
        }) as EventListener;
        manager.addEventListener('auth', handler);
      });

      manager.getPublicKey();
      const pkResult = await pkPromise;

      R.getPublicKeyResult = pkResult.pubkey;
      R.getPublicKeyWorks = R.getPublicKeyResult === R.signerPubkey;

      if (!R.getPublicKeyWorks) {
        R.errors.push(`getPublicKey: expected ${R.signerPubkey}, got ${R.getPublicKeyResult}`);
      } else {
        log('✓ getPublicKey returns consistent pubkey');
      }

      // ---- Test 3: signEvent ----
      statusEl.textContent = 'Test 3: signEvent...';
      log('\n=== Test 3: signEvent ===');

      const template: EventTemplate = {
        kind: 1,
        created_at: Math.floor(Date.now() / 1000),
        tags: [['t', 'worker-wrapper-e2e']],
        content: 'Hello from 4-worker WASM E2E test'
      };

      const signedEvent = await new Promise<NostrEvent>((resolve, reject) => {
        const timeout = setTimeout(() => reject(new Error('Timeout waiting for signed event')), 10000);
        manager.signEvent(template, (event) => {
          clearTimeout(timeout);
          resolve(event);
        });
      });

      R.signedEvent = signedEvent;
      R.signEventWorks = !!signedEvent.pubkey && signedEvent.pubkey === R.signerPubkey;

      if (!R.signEventWorks) {
        R.errors.push(`signEvent: pubkey mismatch. expected=${R.signerPubkey}, got=${signedEvent.pubkey}`);
      } else {
        log(`✓ signEvent returned event with matching pubkey`);
        log(`  id=${signedEvent.id.slice(0, 16)}... sig=${signedEvent.sig.slice(0, 16)}...`);
      }

      // Validate event fields
      if (signedEvent.kind !== template.kind) {
        R.errors.push(`signEvent: kind mismatch. expected=${template.kind}, got=${signedEvent.kind}`);
      }
      if (signedEvent.content !== template.content) {
        R.errors.push(`signEvent: content mismatch`);
      }

      // ---- Test 4: Session persistence ----
      statusEl.textContent = 'Test 4: Session persistence...';
      log('\n=== Test 4: Session Persistence ===');

      // Check localStorage has the session
      const storedPubkey = localStorage.getItem('nostr_active_pubkey');
      const storedAccounts = localStorage.getItem('nostr_signer_accounts');
      R.sessionPersisted = storedPubkey === R.signerPubkey && !!storedAccounts;

      if (!R.sessionPersisted) {
        R.errors.push(`session persistence: storedPubkey=${storedPubkey}, expected=${R.signerPubkey}`);
      } else {
        log('✓ Session persisted to localStorage');
      }

      // Signal that phase 1 is complete; Playwright will reload the page for phase 2
      (window as any).__testPhase1Complete = true;
    }

    // Overall success
    if (isReload) {
      R.success = R.errors.length === 0 && R.sessionRestoredAfterReload && R.signerSet && R.getPublicKeyWorks;
    } else {
      R.success = R.errors.length === 0 && R.subscribeWorks && R.signerSet && R.signEventWorks;
    }
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
