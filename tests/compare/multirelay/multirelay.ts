import type { MultiRelayRunner, MultiRunResult } from './types';

// Multi-relay comparison driver. One contender + one relay set per page load
// (?contender=X&relays=ws://a,ws://b,...&n=2000). Every relay serves the same
// overlapping kinds:[1] set (mock-relay --seed-by filter --unique-fraction
// 0.2); the driver counts unique vs duplicate deliveries, connect-all time,
// all-EOSE time, longtasks, rAF jank and JS heap delta, and reports to
// window.__multiResults.

const params = new URLSearchParams(location.search);
const CONTENDER = params.get('contender') ?? 'nostr-tools';
const RELAYS = (params.get('relays') ?? '')
	.split(',')
	.map((s) => s.trim())
	.filter(Boolean);
const N = Number(params.get('n') ?? '2000');
const RUN_TIMEOUT_MS = Number(params.get('runTimeout') ?? '180000');

interface MultiMetrics {
	contender: string;
	relayCount: number;
	n: number;
	startedAt: string;
	finishedAt?: string;
	perEventWork: string[];
	/** distinct event ids delivered to the consumer */
	uniqueReceived: number;
	/** consumer callbacks fired */
	totalDelivered: number;
	/** callbacks for ids already delivered (duplicates leaked to the app) */
	dupsLeaked: number;
	connectAllMs: number;
	allEoseMs: number;
	firstEventMs: number;
	relaysConnected: number;
	relaysEosed: number;
	wallMs: number;
	uniquePerSec: number;
	deliveredPerSec: number;
	longtaskCount: number;
	longtaskTotalMs: number;
	jankFrames: number;
	jankMs: number;
	heapBeforeBytes: number;
	heapAfterBytes: number;
	heapDeltaBytes: number;
	heapPeakBytes: number;
	timedOut: boolean;
	notes: string[];
	errors: string[];
}

(window as any).__multiResults = null;

const logEl = document.getElementById('log')!;
const statusEl = document.getElementById('status')!;
const log = (s: string) => {
	logEl.textContent += s + '\n';
	console.log(s);
};

async function loadRunner(name: string): Promise<MultiRelayRunner> {
	switch (name) {
		case 'nipworker':
			return (await import('./runners/nipworker')).createNipworkerMultiRunner();
		case 'nostr-tools':
			return (await import('./runners/nostr-tools')).createNostrToolsMultiRunner();
		case 'ndk':
			return (await import('./runners/ndk')).createNdkMultiRunner();
		case 'welshman':
			return (await import('./runners/welshman')).createWelshmanMultiRunner();
		case 'nostrify':
			return (await import('./runners/nostrify')).createNostrifyMultiRunner();
		case 'applesauce':
			return (await import('./runners/applesauce')).createApplesauceMultiRunner();
		case 'innis':
			return (await import('./runners/innis')).createInnisMultiRunner();
		default:
			throw new Error(`unknown contender: ${name}`);
	}
}

function gc() {
	(window as any).gc?.();
}

function heapUsed(): number {
	return (performance as any).memory?.usedJSHeapSize ?? 0;
}

const settle = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function runMulti(): Promise<MultiMetrics> {
	const R: MultiMetrics = {
		contender: CONTENDER,
		relayCount: RELAYS.length,
		n: N,
		startedAt: new Date().toISOString(),
		perEventWork: [],
		uniqueReceived: 0,
		totalDelivered: 0,
		dupsLeaked: 0,
		connectAllMs: -1,
		allEoseMs: -1,
		firstEventMs: -1,
		relaysConnected: 0,
		relaysEosed: 0,
		wallMs: 0,
		uniquePerSec: 0,
		deliveredPerSec: 0,
		longtaskCount: 0,
		longtaskTotalMs: 0,
		jankFrames: 0,
		jankMs: 0,
		heapBeforeBytes: 0,
		heapAfterBytes: 0,
		heapDeltaBytes: 0,
		heapPeakBytes: 0,
		timedOut: false,
		notes: [],
		errors: []
	};

	try {
		statusEl.textContent = `Loading contender: ${CONTENDER}...`;
		const runner = await loadRunner(CONTENDER);
		R.perEventWork = runner.perEventWork;

		statusEl.textContent = `Setting up ${CONTENDER}...`;
		await runner.setup(RELAYS);
		log(`✓ ${CONTENDER} ready, ${RELAYS.length} relays, n=${N} per relay`);

		// --- instrumentation: long tasks -----------------------------------
		const longtasks = { count: 0, totalMs: 0 };
		let po: PerformanceObserver | null = null;
		try {
			po = new PerformanceObserver((list) => {
				for (const entry of list.getEntries()) {
					longtasks.count++;
					longtasks.totalMs += entry.duration;
				}
			});
			po.observe({ entryTypes: ['longtask'] });
		} catch {
			po = null;
		}

		// --- instrumentation: rAF jank proxy ---------------------------------
		let jankFrames = 0;
		let jankMs = 0;
		let rafAlive = true;
		let prevRaf = 0;
		const raf = (t: number) => {
			if (prevRaf > 0) {
				const delta = t - prevRaf;
				if (delta > 50) {
					jankFrames++;
					jankMs += delta - 16.7;
				}
			}
			prevRaf = t;
			if (rafAlive) requestAnimationFrame(raf);
		};
		requestAnimationFrame(raf);

		// --- memory baseline -------------------------------------------------
		gc();
		await settle(50);
		gc();
		const heapBefore = heapUsed();
		let heapPeak = heapBefore;
		const peakTimer = setInterval(() => {
			heapPeak = Math.max(heapPeak, heapUsed());
		}, 50);

		// --- unique/dup accounting -------------------------------------------
		const seen = new Set<string>();
		let total = 0;
		let dups = 0;
		const onEvent = (id: string) => {
			total++;
			if (seen.has(id)) dups++;
			else seen.add(id);
		};

		const subId = `mr_${CONTENDER}_${RELAYS.length}`;
		const t0 = performance.now();
		const result: MultiRunResult = await runner.run(RELAYS, N, subId, onEvent, RUN_TIMEOUT_MS);
		const wallMs = performance.now() - t0;

		clearInterval(peakTimer);
		po?.disconnect();
		rafAlive = false;

		// --- memory after ingestion ------------------------------------------
		await settle(500);
		gc();
		await settle(50);
		gc();
		const heapAfter = heapUsed();
		heapPeak = Math.max(heapPeak, heapUsed());

		R.uniqueReceived = seen.size;
		R.totalDelivered = total;
		R.dupsLeaked = dups;
		R.connectAllMs = result.connectAllMs;
		R.allEoseMs = result.allEoseMs;
		R.firstEventMs = result.firstEventMs;
		R.relaysConnected = result.relaysConnected;
		R.relaysEosed = result.relaysEosed;
		R.wallMs = Math.round(wallMs * 100) / 100;
		R.uniquePerSec = wallMs > 0 ? Math.round(seen.size / (wallMs / 1000)) : 0;
		R.deliveredPerSec = wallMs > 0 ? Math.round(total / (wallMs / 1000)) : 0;
		R.longtaskCount = longtasks.count;
		R.longtaskTotalMs = Math.round(longtasks.totalMs * 100) / 100;
		R.jankFrames = jankFrames;
		R.jankMs = Math.round(jankMs * 100) / 100;
		R.heapBeforeBytes = heapBefore;
		R.heapAfterBytes = heapAfter;
		R.heapDeltaBytes = heapAfter - heapBefore;
		R.heapPeakBytes = heapPeak;
		R.timedOut = result.notes.some((note) => note.startsWith('TIMEOUT'));
		R.notes = result.notes;

		log(
			`${RELAYS.length} relays x ${N}: unique=${R.uniqueReceived}, delivered=${R.totalDelivered}, ` +
				`dupsLeaked=${R.dupsLeaked}, connectAll=${R.connectAllMs}ms, allEose=${R.allEoseMs}ms, ` +
				`wall=${R.wallMs}ms -> ${R.uniquePerSec} uniq ev/s, longtasks=${R.longtaskCount}/${R.longtaskTotalMs}ms, ` +
				`jank=${R.jankFrames}f/${R.jankMs}ms, heapΔ=${(R.heapDeltaBytes / 1048576).toFixed(1)}MB`
		);
		if (R.timedOut) log('⚠ run timed out — partial results recorded');

		await runner.teardown();
		statusEl.textContent = R.errors.length === 0 ? '✅ MULTIRELAY DONE' : '⚠ DONE (with errors)';
	} catch (e: any) {
		R.errors.push(String(e?.message || e));
		statusEl.textContent = '❌ EXCEPTION';
	}

	R.finishedAt = new Date().toISOString();
	(window as any).__multiResults = R;
	return R;
}

runMulti();
