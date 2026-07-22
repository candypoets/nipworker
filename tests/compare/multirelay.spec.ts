import { spawn, type ChildProcess } from 'node:child_process';
import { writeFileSync } from 'node:fs';
import { test, expect } from '@playwright/test';
import WebSocket from 'ws';

// Multi-relay head-to-head: every contender x {5, 10, 25} mock relays, fresh
// page per test; nipworker additionally runs x80 (real apps use ~80 relays).
// The relays are spawned by this spec in --seed-by filter --unique-fraction
// 0.2 mode: all relays serve the same 2,000-event kinds:[1] set with 80%
// byte-identical overlap and a 20% deterministic per-relay unique tail, so
// duplicates arrive across relays exactly like real Nostr. Ports 7721-7745
// serve the first 25 relays, 7801-7855 the remaining 55 (7777/7798/7799 are
// taken by other processes on shared boxes). The single-relay compare suite
// (port 7711, default --seed-by sub mode) is unaffected.

const CONTENDERS = ['nipworker', 'nostr-tools', 'ndk', 'welshman', 'nostrify'];
const RELAY_COUNTS = [5, 10, 25];
const NIPWORKER_RELAY_COUNTS = [5, 10, 25, 80];
const BASE_PORT = 7721;
const EXT_PORT = 7801;
const MAX_RELAYS = 80;
/** Relay ports in spawn/assignment order: 7721-7745, then 7801-7855. */
const RELAY_PORTS = [
	...Array.from({ length: 25 }, (_, i) => BASE_PORT + i),
	...Array.from({ length: MAX_RELAYS - 25 }, (_, i) => EXT_PORT + i)
];
const N = 2000;
const UNIQUE_FRACTION = 0.2;
/** shared part + one unique tail per relay */
const expectedUnique = (r: number) =>
	Math.round(N * (1 - UNIQUE_FRACTION)) + Math.round(N * UNIQUE_FRACTION) * r;
const RUN_TIMEOUT_MS = 180000;

const allResults: Record<string, any> = {};
const children: ChildProcess[] = [];

function spawnRelay(port: number): Promise<ChildProcess> {
	return new Promise((resolve, reject) => {
		const child = spawn(
			process.execPath,
			[
				'tests/bench/mock-relay.mjs',
				'--port',
				String(port),
				'--seed-by',
				'filter',
				'--unique-fraction',
				String(UNIQUE_FRACTION),
				'--seed',
				'777'
			],
			{ stdio: ['ignore', 'pipe', 'pipe'] }
		);
		const timer = setTimeout(() => reject(new Error(`relay :${port} did not start in 15s`)), 15000);
		child.stdout!.on('data', (d) => {
			if (String(d).includes('listening')) {
				clearTimeout(timer);
				resolve(child);
			}
		});
		child.on('error', (err) => {
			clearTimeout(timer);
			reject(err);
		});
		child.on('exit', (code) => {
			clearTimeout(timer);
			reject(new Error(`relay :${port} exited early with code ${code}`));
		});
	});
}

/** First event id served for a kinds:[1] REQ with the given subId (null if unreachable). */
function probeFirstId(port: number, subId: string): Promise<string | null> {
	return new Promise((resolve) => {
		const timer = setTimeout(() => resolve(null), 5000);
		let ws: WebSocket;
		try {
			ws = new WebSocket(`ws://localhost:${port}`);
		} catch {
			clearTimeout(timer);
			return resolve(null);
		}
		ws.on('open', () => ws.send(JSON.stringify(['REQ', subId, { kinds: [1], limit: 5 }])));
		ws.on('message', (d) => {
			const msg = JSON.parse(String(d));
			if (msg[0] === 'EVENT') {
				clearTimeout(timer);
				ws.close();
				resolve(msg[2]?.id ?? null);
			} else if (msg[0] === 'EOSE') {
				clearTimeout(timer);
				ws.close();
				resolve(null);
			}
		});
		ws.on('error', () => {
			clearTimeout(timer);
			resolve(null);
		});
	});
}

/**
 * reuseExistingServer semantics: if something already listens on the port AND
 * behaves like a filter-mode mock relay (same stream for two different
 * subIds), reuse it instead of spawning. Otherwise fail loudly — a sub-mode
 * relay on the port would silently serve non-overlapping events.
 */
async function isReusableMockRelay(port: number): Promise<boolean> {
	const [a, b] = await Promise.all([
		probeFirstId(port, 'probe_a'),
		probeFirstId(port, 'probe_b')
	]);
	return a !== null && a === b;
}

test.beforeAll(async () => {
	test.setTimeout(120000);
	for (let i = 0; i < MAX_RELAYS; i++) {
		const port = RELAY_PORTS[i];
		try {
			children.push(await spawnRelay(port));
		} catch (err) {
			if (await isReusableMockRelay(port)) {
				console.log(`[multirelay] reusing existing filter-mode mock relay on :${port}`);
			} else {
				throw err;
			}
		}
	}
	// Belt and braces: don't leak relays if the runner is interrupted.
	process.on('exit', () => {
		for (const child of children) {
			try {
				child.kill('SIGTERM');
			} catch {
				/* already gone */
			}
		}
	});
	console.log(`[multirelay] ${MAX_RELAYS} mock relays up on :${RELAY_PORTS[0]}-${RELAY_PORTS[MAX_RELAYS - 1]} (${children.length} spawned by this run)`);
});

test.afterAll(() => {
	for (const child of children) {
		try {
			child.kill('SIGTERM');
		} catch {
			/* already gone */
		}
	}
	console.log('\n===== MULTIRELAY SUMMARY (all contenders x relay counts) =====');
	console.log(JSON.stringify(allResults, null, 2));
	try {
		writeFileSync(
			`test-results/multirelay-${Date.now()}.json`,
			JSON.stringify(allResults, null, 2)
		);
	} catch {
		/* result dir not writable; summary is in the log */
	}
});

for (const contender of CONTENDERS) {
	// Only nipworker is benched at x80; the other contenders are hopeless at
	// scale and would just slow the suite down.
	const relayCounts = contender === 'nipworker' ? NIPWORKER_RELAY_COUNTS : RELAY_COUNTS;
	for (const relayCount of relayCounts) {
		test(`multirelay: ${contender} x${relayCount}`, async ({ page }) => {
			test.setTimeout(RUN_TIMEOUT_MS + 180000);

			const relays = Array.from(
				{ length: relayCount },
				(_, i) => `ws://localhost:${RELAY_PORTS[i]}`
			);
			const logs: string[] = [];
			page.on('console', (msg) => logs.push(`[${msg.type()}] ${msg.text()}`));
			page.on('pageerror', (err) => logs.push(`[ERROR] ${err.message}`));

			await page.goto(
				`/tests/compare/multirelay.html?contender=${contender}` +
					`&relays=${encodeURIComponent(relays.join(','))}` +
					`&n=${N}&runTimeout=${RUN_TIMEOUT_MS}`
			);

			await page.waitForFunction(
				() =>
					(window as any).__multiResults !== null &&
					(window as any).__multiResults !== undefined,
				{ timeout: RUN_TIMEOUT_MS + 120000 }
			);

			const r = await page.evaluate(() => (window as any).__multiResults);
			const key = `${contender} x${relayCount}`;
			allResults[key] = r;
			console.log(`\n===== ${key} =====`);
			console.log(JSON.stringify(r, null, 2));

			expect(r.errors, `${key}: driver errors should be empty`).toHaveLength(0);
			expect(r.relaysConnected, `${key}: all relays should connect`).toBe(relayCount);
			if (!r.timedOut) {
				expect(r.relaysEosed, `${key}: all relays should EOSE`).toBe(relayCount);
				expect(
					r.uniqueReceived,
					`${key}: every unique event must reach the app (${expectedUnique(relayCount)} expected)`
				).toBe(expectedUnique(relayCount));
				// Libs with real cross-relay dedup must not leak a single duplicate.
				if (['nipworker', 'nostr-tools', 'ndk'].includes(contender)) {
					expect(r.dupsLeaked, `${key}: ${contender} should not leak duplicates`).toBe(0);
				}
			} else {
				// Hopeless at scale: partial results are recorded, not a hang.
				expect(r.uniqueReceived, `${key}: timed-out run should still have delivered events`).toBeGreaterThan(0);
				console.log(`⚠ ${key}: TIMED OUT — partial results recorded`);
			}
			console.log('Browser logs:');
			logs.forEach((l) => console.log(l));
		});
	}
}
