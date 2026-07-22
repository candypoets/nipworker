import type { ContenderRunner, RunResult } from './runners/types';

// Head-to-head comparison driver. One contender per page load (?contender=X),
// runs the same ingest scenario for N in {1000, 10000} against the mock relay
// and reports to window.__compareResults.

const params = new URLSearchParams(location.search);
const CONTENDER = params.get('contender') ?? 'nostr-tools';
const RELAY = params.get('relay') ?? 'ws://localhost:7711';
const NS = (params.get('n') ?? '1000,10000')
	.split(',')
	.map((s) => Number(s.trim()))
	.filter((n) => n > 0);
const RUN_TIMEOUT_MS = Number(params.get('runTimeout') ?? '240000');

interface RunMetrics {
	n: number;
	received: number;
	rawCount: number;
	wallMs: number;
	eventsPerSec: number;
	firstEventMs: number;
	longtaskCount: number;
	longtaskTotalMs: number;
	jankFrames: number;
	jankMs: number;
	heapBeforeBytes: number;
	heapAfterBytes: number;
	heapDeltaBytes: number;
	heapPeakBytes: number;
	notes: string[];
}

interface CompareResults {
	contender: string;
	relay: string;
	startedAt: string;
	finishedAt?: string;
	perEventWork: string[];
	runs: RunMetrics[];
	errors: string[];
}

(window as any).__compareResults = null;

const logEl = document.getElementById('log')!;
const statusEl = document.getElementById('status')!;
const log = (s: string) => {
	logEl.textContent += s + '\n';
	console.log(s);
};

async function loadRunner(name: string): Promise<ContenderRunner> {
	switch (name) {
		case 'nipworker':
			return (await import('./runners/nipworker')).createNipworkerRunner();
		case 'nostr-tools':
			return (await import('./runners/nostr-tools')).createNostrToolsRunner();
		case 'ndk':
			return (await import('./runners/ndk')).createNdkRunner();
		case 'welshman':
			return (await import('./runners/welshman')).createWelshmanRunner();
		case 'nostrify':
			return (await import('./runners/nostrify')).createNostrifyRunner();
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

async function measureRun(
	runner: ContenderRunner,
	n: number
): Promise<RunMetrics> {
	// --- instrumentation: long tasks -------------------------------------
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
		po = null; // longtask not supported
	}

	// --- instrumentation: rAF jank proxy ----------------------------------
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

	// --- memory baseline --------------------------------------------------
	gc();
	await settle(50);
	gc();
	const heapBefore = heapUsed();
	let heapPeak = heapBefore;
	const peakTimer = setInterval(() => {
		heapPeak = Math.max(heapPeak, heapUsed());
	}, 50);

	const subId = `cmp_${CONTENDER}_${n}`;
	const t0 = performance.now();
	let firstEventMs = 0;
	let lastEventTs = 0;

	const runPromise = runner.run(RELAY, n, subId, () => {
		const now = performance.now();
		if (firstEventMs === 0) firstEventMs = now - t0;
		lastEventTs = now;
	});
	const timeoutPromise = new Promise<RunResult>((resolve) =>
		setTimeout(
			() =>
				resolve({
					received: -1,
					rawCount: -1,
					notes: [`TIMEOUT after ${RUN_TIMEOUT_MS}ms`]
				}),
			RUN_TIMEOUT_MS
		)
	);
	const result = await Promise.race([runPromise, timeoutPromise]);

	clearInterval(peakTimer);
	po?.disconnect();
	rafAlive = false;

	// --- memory after ingestion -------------------------------------------
	await settle(500);
	gc();
	await settle(50);
	gc();
	const heapAfter = heapUsed();
	heapPeak = Math.max(heapPeak, heapUsed());

	const wallMs = (lastEventTs || performance.now()) - t0;
	const received = result.received;
	return {
		n,
		received,
		rawCount: result.rawCount,
		wallMs: Math.round(wallMs * 100) / 100,
		eventsPerSec: wallMs > 0 && received > 0 ? Math.round(received / (wallMs / 1000)) : 0,
		firstEventMs: Math.round(firstEventMs * 100) / 100,
		longtaskCount: longtasks.count,
		longtaskTotalMs: Math.round(longtasks.totalMs * 100) / 100,
		jankFrames,
		jankMs: Math.round(jankMs * 100) / 100,
		heapBeforeBytes: heapBefore,
		heapAfterBytes: heapAfter,
		heapDeltaBytes: heapAfter - heapBefore,
		heapPeakBytes: heapPeak,
		notes: result.notes
	};
}

async function runCompare(): Promise<CompareResults> {
	const R: CompareResults = {
		contender: CONTENDER,
		relay: RELAY,
		startedAt: new Date().toISOString(),
		perEventWork: [],
		runs: [],
		errors: []
	};

	try {
		statusEl.textContent = `Loading contender: ${CONTENDER}...`;
		const runner = await loadRunner(CONTENDER);
		R.perEventWork = runner.perEventWork;

		statusEl.textContent = `Setting up ${CONTENDER}...`;
		await runner.setup(RELAY);
		log(`✓ ${CONTENDER} ready, relay=${RELAY}`);

		for (const n of NS) {
			statusEl.textContent = `${CONTENDER}: ingesting ${n} events...`;
			try {
				const m = await measureRun(runner, n);
				R.runs.push(m);
				log(
					`n=${n}: ${m.received} events in ${m.wallMs}ms -> ${m.eventsPerSec} ev/s, ` +
						`first=${m.firstEventMs}ms, longtasks=${m.longtaskCount}/${m.longtaskTotalMs}ms, ` +
						`jank=${m.jankFrames}f/${m.jankMs}ms, heapΔ=${(m.heapDeltaBytes / 1048576).toFixed(1)}MB ` +
						`(peak ${(m.heapPeakBytes / 1048576).toFixed(1)}MB)`
				);
				if (m.received >= 0 && m.received < n) {
					R.errors.push(`n=${n}: only ${m.received}/${n} events received`);
				}
				if (m.received === -1) R.errors.push(`n=${n}: run timed out`);
			} catch (e: any) {
				R.errors.push(`n=${n}: ${e?.message || e}`);
			}
		}

		await runner.teardown();
		statusEl.textContent = R.errors.length === 0 ? '✅ COMPARE DONE' : '⚠ COMPARE DONE (with errors)';
	} catch (e: any) {
		R.errors.push(String(e?.message || e));
		statusEl.textContent = '❌ EXCEPTION';
	}

	R.finishedAt = new Date().toISOString();
	(window as any).__compareResults = R;
	return R;
}

runCompare();
