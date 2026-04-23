import { createNostrManager, setManager } from '../../src/index';
import { useSubscription } from '../../src/hooks';
import { WorkerMessage, MessageType, ParsedEvent } from '../../src/generated/nostr/fb';

const RELAY = 'wss://nos.lol';
const FILTER = { kinds: [1], limit: 10, relays: [RELAY] };

interface MsgRecord {
  type: string;
  ts: number;
  event?: ParsedEvent;
}

interface CacheTestResult {
  name: string;
  filter: any;
  events: number;
  duration: number;
  passed: boolean;
  error?: string;
}

interface TestResults {
  success: boolean;
  sub1: MsgRecord[];
  sub2: MsgRecord[];
  relayStatuses: { url: string; status: string; ts: number }[];
  cacheHit: boolean;
  cacheTests: CacheTestResult[];
  errors: string[];
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

function extractEvent(msg: WorkerMessage): ParsedEvent | null {
  if (msg.type() === MessageType.ParsedNostrEvent) {
    return msg.content(new ParsedEvent()) as ParsedEvent;
  }
  return null;
}

async function runCacheOnlyQuery(
  name: string,
  filter: any,
  timeoutMs: number = 500
): Promise<CacheTestResult> {
  const records: MsgRecord[] = [];
  const start = performance.now();
  let eoceTime: number | null = null;

  await new Promise<void>((resolve) => {
    const unsub = useSubscription(
      `cache_test_${name}`,
      [{ ...filter, relays: [RELAY] }],
      (msg) => {
        const record: MsgRecord = { type: typeName(msg), ts: Date.now() };
        const event = extractEvent(msg);
        if (event) {
          record.event = event;
        }
        records.push(record);
        if (record.type === 'Eoce') {
          eoceTime = performance.now() - start;
          unsub();
          resolve();
        }
      },
      { closeOnEose: true, bytesPerEvent: 8192, cacheOnly: true }
    );
    setTimeout(() => { unsub(); resolve(); }, timeoutMs);
  });

  const duration = eoceTime ?? Math.round(performance.now() - start);
  const events = records.filter(m => m.type === 'ParsedNostrEvent');
  const eoce = records.find(m => m.type === 'Eoce');
  const hasEoce = eoce !== undefined;
  const passed = hasEoce;

  return {
    name,
    filter,
    events: events.length,
    duration,
    passed,
    error: passed ? undefined : `no EOCE received (events=${events.length})`
  };
}

async function runTest(): Promise<TestResults> {
  const R: TestResults = {
    success: false,
    sub1: [],
    sub2: [],
    relayStatuses: [],
    cacheHit: false,
    cacheTests: [],
    errors: []
  };
  const logEl = document.getElementById('log')!;
  const statusEl = document.getElementById('status')!;
  const log = (s: string) => { logEl.textContent += s + '\n'; console.log(s); };

  try {
    statusEl.textContent = 'Booting 4-worker WASM...';
    const manager = createNostrManager();
    setManager(manager);

    manager.addEventListener('relay:status', ((evt: CustomEvent) => {
      const { url, status } = evt.detail;
      R.relayStatuses.push({ url, status, ts: Date.now() });
      log(`[relay] ${url} -> ${status}`);
    }) as EventListener);

    log('✓ NostrManager (4-worker) ready');

    await new Promise(r => setTimeout(r, 1500));

    // ---- Sub1: first subscription, populates cache ----
    statusEl.textContent = 'Sub1 running (network)...';
    const sub1Start = performance.now();
    await new Promise<void>((resolve) => {
      const unsub = useSubscription('sub1', [FILTER], (msg) => {
        const record: MsgRecord = { type: typeName(msg), ts: Date.now() };
        const event = extractEvent(msg);
        if (event) record.event = event;
        R.sub1.push(record);
      }, { closeOnEose: true, bytesPerEvent: 8192 });
      setTimeout(() => { unsub(); resolve(); }, 15000);
    });
    const sub1Duration = performance.now() - sub1Start;
    log(`sub1: ${R.sub1.length} messages (${sub1Duration.toFixed(0)}ms)`);
    R.sub1.forEach(m => log(`  [sub1] ${m.type}`));

    const sub1Events = R.sub1.filter(m => m.type === 'ParsedNostrEvent');
    if (sub1Events.length === 0) {
      R.errors.push('sub1: no ParsedNostrEvent from network');
    }

    const uniqueAuthors = [...new Set(sub1Events
      .map(m => m.event?.pubkey())
      .filter((p): p is string => !!p))];

    const uniqueKinds = [...new Set(sub1Events
      .map(m => m.event?.kind())
      .filter((k): k is number => k !== undefined))];

    const eTags: string[] = [];
    const pTags: string[] = [];
    const tTags: string[] = [];

    sub1Events.forEach(m => {
      const event = m.event;
      if (!event) return;
      const tags = event.tags();
      for (let i = 0; i < tags.length; i++) {
        const tag = tags.get(i);
        const items = tag.items();
        if (items.length >= 2) {
          const tagName = items.get(0);
          const tagValue = items.get(1);
          if (tagName === 'e') eTags.push(tagValue);
          if (tagName === 'p') pTags.push(tagValue);
          if (tagName === 't') tTags.push(tagValue);
        }
      }
    });

    log(`Extracted from sub1: ${uniqueAuthors.length} authors, kinds: [${uniqueKinds.join(',')}]`);
    log(`  e-tags: ${eTags.length}, p-tags: ${pTags.length}, t-tags: ${tTags.length}`);

    log('Waiting 3s for persistence...');
    await new Promise(r => setTimeout(r, 3000));

    // ---- Run multiple cache-only tests with different filters ----
    statusEl.textContent = 'Running cache-only filter tests...';
    log('\n=== Cache-Only Filter Tests ===');

    const cacheTests: CacheTestResult[] = [];

    cacheTests.push(await runCacheOnlyQuery('kind_1_limit_10', { kinds: [1], limit: 10 }));
    cacheTests.push(await runCacheOnlyQuery('kind_1_limit_3', { kinds: [1], limit: 3 }));
    cacheTests.push(await runCacheOnlyQuery('kind_1_no_limit', { kinds: [1] }));

    if (uniqueAuthors.length > 0) {
      cacheTests.push(await runCacheOnlyQuery(
        'by_author_0',
        { authors: [uniqueAuthors[0]], limit: 5 }
      ));
      if (uniqueAuthors.length > 1) {
        cacheTests.push(await runCacheOnlyQuery(
          'by_authors_multi',
          { authors: uniqueAuthors.slice(0, 3), limit: 10 }
        ));
      }
    }

    if (uniqueAuthors.length > 0 && uniqueKinds.length > 0) {
      cacheTests.push(await runCacheOnlyQuery(
        'kind_author_combo',
        { kinds: [uniqueKinds[0]], authors: [uniqueAuthors[0]], limit: 5 }
      ));
    }

    cacheTests.push(await runCacheOnlyQuery('nonexistent_kind', { kinds: [99999], limit: 5 }));
    cacheTests.push(await runCacheOnlyQuery(
      'nonexistent_author',
      { authors: ['0000000000000000000000000000000000000000000000000000000000000000'], limit: 5 }
    ));

    if (pTags.length > 0) {
      cacheTests.push(await runCacheOnlyQuery('by_p_tag', { '#p': [pTags[0]], limit: 5 }));
    }
    if (eTags.length > 0) {
      cacheTests.push(await runCacheOnlyQuery('by_e_tag', { '#e': [eTags[0]], limit: 5 }));
    }

    const oneHourAgo = Math.floor(Date.now() / 1000) - 3600;
    cacheTests.push(await runCacheOnlyQuery('since_recent', { kinds: [1], since: oneHourAgo, limit: 10 }));

    const futureTime = Math.floor(Date.now() / 1000) + 1000000;
    cacheTests.push(await runCacheOnlyQuery('since_future', { kinds: [1], since: futureTime, limit: 5 }));

    cacheTests.push(await runCacheOnlyQuery('shard_kind0', { kinds: [0], limit: 5 }));
    cacheTests.push(await runCacheOnlyQuery('shard_kind4', { kinds: [4], limit: 5 }));
    cacheTests.push(await runCacheOnlyQuery('shard_kind7375', { kinds: [7375], limit: 5 }));
    cacheTests.push(await runCacheOnlyQuery('shard_kind10002', { kinds: [10002], limit: 5 }));

    R.cacheTests = cacheTests;

    cacheTests.forEach(t => {
      const status = t.passed ? '✓' : '✗';
      log(`${status} ${t.name}: ${t.events} events in ${t.duration}ms (${t.passed ? 'PASS' : 'FAIL'})`);
      if (t.error) log(`    Error: ${t.error}`);
    });

    const passedTests = cacheTests.filter(t => t.passed).length;
    log(`\nCache tests: ${passedTests}/${cacheTests.length} passed`);

    // ---- Sub2: original cache test (same filter) ----
    statusEl.textContent = 'Sub2 running (cache + resubscription)...';
    const sub2Start = performance.now();
    await new Promise<void>((resolve) => {
      const unsub = useSubscription('sub2', [FILTER], (msg) => {
        R.sub2.push({ type: typeName(msg), ts: Date.now() });
      }, { closeOnEose: true, bytesPerEvent: 8192, cacheOnly: true });
      setTimeout(() => { unsub(); resolve(); }, 15000);
    });
    const sub2Duration = performance.now() - sub2Start;
    log(`\nsub2: ${R.sub2.length} messages (${sub2Duration.toFixed(0)}ms)`);
    R.sub2.forEach(m => log(`  [sub2] ${m.type}`));

    const sub2Events = R.sub2.filter(m => m.type === 'ParsedNostrEvent').length;

    const parsedIdx = R.sub2.findIndex(m => m.type === 'ParsedNostrEvent');
    const eoceIdx   = R.sub2.findIndex(m => m.type === 'Eoce');
    if (parsedIdx !== -1 && eoceIdx !== -1 && parsedIdx < eoceIdx) {
      R.cacheHit = true;
      log('✓ CACHE HIT: ParsedNostrEvent before Eoce');
    } else if (sub2Events > 0) {
      log(`⚠ Cache timing: parsedIdx=${parsedIdx} eoceIdx=${eoceIdx} (events arrived, but after Eoce)`);
    } else {
      log(`ℹ Cache miss or empty: sub2 got ${sub2Events} events`);
    }

    const allCacheTestsPassed = cacheTests.every(t => t.passed);
    R.success = sub1Events.length > 0 && allCacheTestsPassed && R.errors.length === 0;
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
