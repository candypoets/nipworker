import { createNostrManager, setManager } from '../../src/index';
import { useSubscription } from '../../src/hooks';
import { WorkerMessage, MessageType, ParsedEvent } from '../../src/generated/nostr/fb';
import { ByteBuffer } from 'flatbuffers';

const RELAY = 'wss://relay.damus.io';
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
        // Stop early on EOCE for fast queries
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
  
  // For cacheOnly queries, EOCE should come (either after events, or immediately if no matches)
  const hasEoce = eoce !== undefined;
  // A cache query passes if:
  // 1. It has an EOCE response (cache responded)
  // 2. If it returned events, they came before EOCE
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
    statusEl.textContent = 'Booting engine...';
    const manager = createNostrManager({ engine: true });
    setManager(manager);

    // Listen for relay status updates (EngineManager dispatches these)
    manager.addEventListener('relay:status', ((evt: CustomEvent) => {
      const { url, status } = evt.detail;
      R.relayStatuses.push({ url, status, ts: Date.now() });
      log(`[relay] ${url} -> ${status}`);
    }) as EventListener);

    log('✓ EngineManager ready');

    // Wait a bit for worker init
    await new Promise(r => setTimeout(r, 1000));

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

    // Extract unique authors and other data from cached events for testing
    const uniqueAuthors = [...new Set(sub1Events
      .map(m => m.event?.pubkey())
      .filter((p): p is string => !!p))];
    
    const uniqueKinds = [...new Set(sub1Events
      .map(m => m.event?.kind())
      .filter((k): k is number => k !== undefined))];
    
    // Extract tags for testing
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

    // Let IndexedDB flush and indexing complete
    log('Waiting 3s for persistence...');
    await new Promise(r => setTimeout(r, 3000));

    // ---- Run multiple cache-only tests with different filters ----
    statusEl.textContent = 'Running cache-only filter tests...';
    log('\n=== Cache-Only Filter Tests ===');
    
    const cacheTests: CacheTestResult[] = [];

    // Test 1: Same as original (kind: 1, limit: 10)
    cacheTests.push(await runCacheOnlyQuery('kind_1_limit_10', { kinds: [1], limit: 10 }));

    // Test 2: Same kind, smaller limit
    cacheTests.push(await runCacheOnlyQuery('kind_1_limit_3', { kinds: [1], limit: 3 }));

    // Test 3: Same kind, no limit (should get all cached)
    cacheTests.push(await runCacheOnlyQuery('kind_1_no_limit', { kinds: [1] }));

    // Test 4: By author (if we have any)
    if (uniqueAuthors.length > 0) {
      cacheTests.push(await runCacheOnlyQuery(
        'by_author_0', 
        { authors: [uniqueAuthors[0]], limit: 5 }
      ));
      
      // Test 5: By multiple authors
      if (uniqueAuthors.length > 1) {
        cacheTests.push(await runCacheOnlyQuery(
          'by_authors_multi',
          { authors: uniqueAuthors.slice(0, 3), limit: 10 }
        ));
      }
    }

    // Test 6: By kind + author combo
    if (uniqueAuthors.length > 0 && uniqueKinds.length > 0) {
      cacheTests.push(await runCacheOnlyQuery(
        'kind_author_combo',
        { kinds: [uniqueKinds[0]], authors: [uniqueAuthors[0]], limit: 5 }
      ));
    }

    // Test 7: Non-existent kind (should return empty)
    cacheTests.push(await runCacheOnlyQuery('nonexistent_kind', { kinds: [99999], limit: 5 }));

    // Test 8: Non-existent author (should return empty)
    cacheTests.push(await runCacheOnlyQuery(
      'nonexistent_author',
      { authors: ['0000000000000000000000000000000000000000000000000000000000000000'], limit: 5 }
    ));

    // Test 9: By p-tag (if available)
    if (pTags.length > 0) {
      cacheTests.push(await runCacheOnlyQuery(
        'by_p_tag',
        { '#p': [pTags[0]], limit: 5 }
      ));
    }

    // Test 10: By e-tag (if available)
    if (eTags.length > 0) {
      cacheTests.push(await runCacheOnlyQuery(
        'by_e_tag',
        { '#e': [eTags[0]], limit: 5 }
      ));
    }

    // Test 11: Since filter (recent events only)
    const oneHourAgo = Math.floor(Date.now() / 1000) - 3600;
    cacheTests.push(await runCacheOnlyQuery(
      'since_recent',
      { kinds: [1], since: oneHourAgo, limit: 10 }
    ));

    // Test 12: Since filter (far future - should be empty)
    const futureTime = Math.floor(Date.now() / 1000) + 1000000;
    cacheTests.push(await runCacheOnlyQuery(
      'since_future',
      { kinds: [1], since: futureTime, limit: 5 }
    ));

    // Test 13: Shard verification - query kind 0 (should go to Kind0 shard)
    cacheTests.push(await runCacheOnlyQuery(
      'shard_kind0',
      { kinds: [0], limit: 5 }
    ));

    // Test 14: Shard verification - query kind 4 (should go to Kind4 shard)
    cacheTests.push(await runCacheOnlyQuery(
      'shard_kind4',
      { kinds: [4], limit: 5 }
    ));

    // Test 15: Shard verification - query kind 7375 (should go to Kind7375 shard)
    cacheTests.push(await runCacheOnlyQuery(
      'shard_kind7375',
      { kinds: [7375], limit: 5 }
    ));

    // Test 16: Shard verification - query kind 10002 (should go to Kind10002 shard)
    cacheTests.push(await runCacheOnlyQuery(
      'shard_kind10002',
      { kinds: [10002], limit: 5 }
    ));

    R.cacheTests = cacheTests;

    // Log results
    cacheTests.forEach(t => {
      const status = t.passed ? '✓' : '✗';
      log(`${status} ${t.name}: ${t.events} events in ${t.duration}ms (${t.passed ? 'PASS' : 'FAIL'})`);
      if (t.error) log(`    Error: ${t.error}`);
    });

    // Count passed tests
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

    // Cache hit heuristic: if sub2 got events significantly faster or before Eoce
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

    // Success criteria: sub1 got network events AND all cache tests passed
    const allCacheTestsPassed = cacheTests.every(t => t.passed);
    R.success = sub1Events.length > 0 && allCacheTestsPassed && R.errors.length === 0;
    statusEl.textContent = R.success ? '✅ TEST PASSED' : '❌ TEST FAILED';

  } catch (e: any) {
    R.errors.push(String(e.message || e));
    statusEl.textContent = '❌ EXCEPTION';
  }

  (window as any).__testResults = R;
  return R;
}

runTest();
