import { createNostrManager, setManager } from '../../src/index';
import { useSubscription, isParsedEvent, isEoce } from '../../src/hooks';
import type { ParsedEvent } from '../../src/generated/nostr/fb';

const params = new URLSearchParams(location.search);
const RELAY = params.get('relay') ?? 'ws://localhost:7710';
const LIVE_EXPECTED = Number(params.get('live') ?? '200');

interface ThroughputResult {
	events: number;
	received: number;
	wallMs: number;
	eventsPerSec: number;
	firstEventMs: number;
	batches: number;
	avgBatchSize: number;
}

interface CacheQueryStats {
	limit: number;
	repeats: number;
	samples: number[];
	eventsPerQuery: number;
	mean: number;
	p50: number;
	p95: number;
	p99: number;
	min: number;
	max: number;
}

interface BenchResults {
	relay: string;
	startedAt: string;
	finishedAt?: string;
	throughput: ThroughputResult[];
	cacheQueryLatency: CacheQueryStats[];
	e2eLatency: {
		reqToFirstEventMs: number;
		reqToLastCachedEventMs: number;
		live: {
			expected: number;
			received: number;
			avgMs: number;
			p50Ms: number;
			p95Ms: number;
			maxMs: number;
			eventsPerSec: number;
		};
	} | null;
	errors: string[];
}

(window as any).__benchResults = null;

const logEl = document.getElementById('log')!;
const statusEl = document.getElementById('status')!;
const log = (s: string) => {
	logEl.textContent += s + '\n';
	console.log(s);
};

function percentile(sorted: number[], p: number): number {
	if (sorted.length === 0) return 0;
	const idx = Math.min(sorted.length - 1, Math.ceil((p / 100) * sorted.length) - 1);
	return sorted[Math.max(0, idx)];
}

function stats(samples: number[]) {
	const sorted = [...samples].sort((a, b) => a - b);
	const mean = samples.reduce((a, b) => a + b, 0) / (samples.length || 1);
	return {
		mean: Math.round(mean * 100) / 100,
		p50: percentile(sorted, 50),
		p95: percentile(sorted, 95),
		p99: percentile(sorted, 99),
		min: sorted[0] ?? 0,
		max: sorted[sorted.length - 1] ?? 0
	};
}

function benchTs(ev: ParsedEvent): number | null {
	for (let i = 0; i < ev.tagsLength(); i++) {
		const tag = ev.tags(i);
		if (tag && tag.itemsLength() >= 2 && tag.items(0) === 'bt') {
			const ms = Number(tag.items(1));
			return Number.isFinite(ms) ? ms : null;
		}
	}
	return null;
}

// ---- Phase 1: throughput -------------------------------------------------
function runThroughput(n: number): Promise<ThroughputResult> {
	return new Promise((resolve) => {
		const subId = `bench_tp_${n}`;
		let received = 0;
		let batches = 0;
		let prevTs = 0;
		let firstEventMs = 0;
		let lastEventTs = 0;
		const start = performance.now();

		const finish = () => {
			unsub();
			const wallMs = (lastEventTs || performance.now()) - start;
			resolve({
				events: n,
				received,
				wallMs: Math.round(wallMs * 100) / 100,
				eventsPerSec: wallMs > 0 ? Math.round(received / (wallMs / 1000)) : 0,
				firstEventMs: Math.round(firstEventMs * 100) / 100,
				batches,
				avgBatchSize: batches > 0 ? Math.round((received / batches) * 100) / 100 : received
			});
		};

		const timer = setTimeout(finish, 90000);
		const unsub = useSubscription(
			subId,
			[{ kinds: [1], limit: n, relays: [RELAY] }],
			(msg) => {
				if (!isParsedEvent(msg)) return;
				const now = performance.now();
				received++;
				if (received === 1) firstEventMs = now - start;
				// Approximate delivery batches: a >1ms gap between consecutive
				// messages means a new parser->main frame arrived.
				if (now - prevTs > 1) batches++;
				prevTs = now;
				lastEventTs = now;
				if (received >= n) {
					clearTimeout(timer);
					// Let the callback loop drain before unsubscribing.
					setTimeout(finish, 0);
				}
			},
			{ closeOnEose: true, bytesPerEvent: 8192, skipCache: true }
		);
	});
}

// ---- Phase 2: cache-only query latency ------------------------------------
function runCacheQuery(limit: number, repeat: number): Promise<{ ms: number; events: number }> {
	return new Promise((resolve) => {
		const subId = `bench_cache_${limit}_${repeat}`;
		let events = 0;
		const start = performance.now();
		let done = false;
		const finish = (ms: number) => {
			if (done) return;
			done = true;
			unsub();
			resolve({ ms: Math.round(ms * 100) / 100, events });
		};
		const unsub = useSubscription(
			subId,
			[{ kinds: [1], limit, relays: [RELAY] }],
			(msg) => {
				if (isParsedEvent(msg)) {
					events++;
					return;
				}
				if (isEoce(msg)) {
					finish(performance.now() - start);
				}
			},
			{ closeOnEose: true, bytesPerEvent: 8192, cacheOnly: true }
		);
		setTimeout(() => finish(performance.now() - start), 3000);
	});
}

async function runCachePhase(repeats: number): Promise<CacheQueryStats[]> {
	const out: CacheQueryStats[] = [];
	for (const limit of [20, 100, 1000]) {
		const samples: number[] = [];
		let eventsPerQuery = 0;
		for (let i = 0; i < repeats; i++) {
			const r = await runCacheQuery(limit, i);
			samples.push(r.ms);
			eventsPerQuery = r.events;
		}
		out.push({ limit, repeats, samples, eventsPerQuery, ...stats(samples) });
		log(`cache limit=${limit}: p50=${out[out.length - 1].p50}ms p95=${out[out.length - 1].p95}ms (${eventsPerQuery} events/query)`);
	}
	return out;
}

// ---- Phase 3: end-to-end latency ------------------------------------------
function runE2ELatency(): Promise<BenchResults['e2eLatency']> {
	return new Promise((resolve) => {
		const subId = 'bench_e2e';
		const start = performance.now();
		let firstEventMs = 0;
		let lastCachedMs = 0;
		let seenLive = false;
		let firstLiveTs = 0;
		let lastLiveTs = 0;
		const latencies: number[] = [];

		const finish = () => {
			unsub();
			const s = stats(latencies);
			resolve({
				reqToFirstEventMs: Math.round(firstEventMs * 100) / 100,
				reqToLastCachedEventMs: Math.round(lastCachedMs * 100) / 100,
				live: {
					expected: LIVE_EXPECTED,
					received: latencies.length,
					avgMs: s.mean,
					p50Ms: s.p50,
					p95Ms: s.p95,
					maxMs: s.max,
					eventsPerSec:
						firstLiveTs > 0 && lastLiveTs > firstLiveTs
							? Math.round(latencies.length / ((lastLiveTs - firstLiveTs) / 1000))
							: 0
				}
			});
		};

		const timer = setTimeout(finish, 15000);
		const unsub = useSubscription(
			subId,
			[{ kinds: [1], limit: 250, relays: [RELAY] }],
			(msg) => {
				const ev = isParsedEvent(msg);
				if (!ev) return;
				const now = performance.now();
				const bt = benchTs(ev);
				if (bt !== null) {
					if (!seenLive) {
						seenLive = true;
						firstLiveTs = now;
					}
					lastLiveTs = now;
					latencies.push(Date.now() - bt);
					if (latencies.length >= LIVE_EXPECTED) {
						clearTimeout(timer);
						setTimeout(finish, 0);
					}
				} else {
					if (firstEventMs === 0) firstEventMs = now - start;
					lastCachedMs = now - start;
				}
			},
			// limit 250 sizes the subscription ring buffer (~250 * bytesPerEvent)
			// so the post-EOSE live burst does not overflow it.
			{ closeOnEose: false, bytesPerEvent: 8192, skipCache: true }
		);
	});
}

async function runBench(): Promise<BenchResults> {
	const R: BenchResults = {
		relay: RELAY,
		startedAt: new Date().toISOString(),
		throughput: [],
		cacheQueryLatency: [],
		e2eLatency: null,
		errors: []
	};

	try {
		statusEl.textContent = 'Booting 4-worker WASM...';
		const manager = createNostrManager();
		setManager(manager);
		(window as any).__benchManager = manager;
		manager.addEventListener('relay:status', ((evt: CustomEvent) => {
			log(`[relay] ${evt.detail.url} -> ${evt.detail.status}`);
		}) as EventListener);
		log(`✓ NostrManager ready, relay=${RELAY}`);
		await new Promise((r) => setTimeout(r, 1000));

		// Phase 1: throughput for 100 / 1000 / 10000 events.
		for (const n of [100, 1000, 10000]) {
			statusEl.textContent = `Throughput run: ${n} events...`;
			try {
				const r = await runThroughput(n);
				R.throughput.push(r);
				log(`throughput n=${n}: ${r.received} events in ${r.wallMs}ms -> ${r.eventsPerSec} ev/s, first=${r.firstEventMs}ms, batches~${r.batches}`);
				if (r.received < n) R.errors.push(`throughput n=${n}: only ${r.received}/${n} events received`);
			} catch (e: any) {
				R.errors.push(`throughput n=${n}: ${e?.message || e}`);
			}
		}

		// Let the cache worker persist the 10000-event run.
		log('Waiting 5s for cache persistence...');
		await new Promise((r) => setTimeout(r, 5000));

		// Phase 2: cache-only query latency.
		statusEl.textContent = 'Cache query latency...';
		try {
			R.cacheQueryLatency = await runCachePhase(20);
		} catch (e: any) {
			R.errors.push(`cacheQueryLatency: ${e?.message || e}`);
		}

		// Phase 3: end-to-end latency (REQ -> first event, live burst).
		statusEl.textContent = 'E2E latency...';
		try {
			R.e2eLatency = await runE2ELatency();
			if (R.e2eLatency) {
				log(
					`e2e: first=${R.e2eLatency.reqToFirstEventMs}ms live avg=${R.e2eLatency.live.avgMs}ms (${R.e2eLatency.live.received}/${R.e2eLatency.live.expected})`
				);
			}
		} catch (e: any) {
			R.errors.push(`e2eLatency: ${e?.message || e}`);
		}

		statusEl.textContent = R.errors.length === 0 ? '✅ BENCH DONE' : '⚠ BENCH DONE (with errors)';
	} catch (e: any) {
		R.errors.push(String(e?.message || e));
		statusEl.textContent = '❌ EXCEPTION';
	}

	R.finishedAt = new Date().toISOString();
	(window as any).__benchResults = R;
	return R;
}

runBench();
