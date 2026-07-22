import { test, expect } from '@playwright/test';

// Each contender runs in its own test => its own fresh page => uncontaminated
// heap measurements. The compare page runs the same scenario (kinds:[1],
// limit n in {1000, 10000}) against the deterministic mock relay.

const CONTENDERS = ['nipworker', 'nostr-tools', 'ndk', 'welshman', 'nostrify'];
const RELAY = 'ws://localhost:7711';
const EXPECTED = [1000, 10000];

const allResults: Record<string, any> = {};

for (const contender of CONTENDERS) {
	test(`compare: ${contender}`, async ({ page }) => {
		test.setTimeout(420000);

		const logs: string[] = [];
		page.on('console', (msg) => logs.push(`[${msg.type()}] ${msg.text()}`));
		page.on('pageerror', (err) => logs.push(`[ERROR] ${err.message}`));

		await page.goto(`/tests/compare/compare.html?contender=${contender}&relay=${encodeURIComponent(RELAY)}`);

		await page.waitForFunction(
			() =>
				(window as any).__compareResults !== null &&
				(window as any).__compareResults !== undefined,
			{ timeout: 400000 }
		);

		const results = await page.evaluate(() => (window as any).__compareResults);

		// Renderer-level CDP metrics (best-effort extra memory signal).
		let cdpMetrics: Record<string, number> = {};
		try {
			const session = await page.context().newCDPSession(page);
			await session.send('Performance.enable');
			const { metrics } = await session.send('Performance.getMetrics');
			for (const m of metrics) {
				if (
					['JSHeapUsedSize', 'JSHeapTotalSize', 'Documents', 'Nodes', 'JSEventListeners'].includes(
						m.name
					)
				) {
					cdpMetrics[m.name] = m.value;
				}
			}
		} catch {
			/* CDP not available */
		}

		allResults[contender] = { ...results, cdpMetrics };
		console.log(`\n===== ${contender} =====`);
		console.log(JSON.stringify({ ...results, cdpMetrics }, null, 2));
		console.log('Browser logs:');
		logs.forEach((l) => console.log(l));

		expect(results.errors, `${contender}: compare errors should be empty`).toHaveLength(0);
		expect(results.runs.length, `${contender}: should have ${EXPECTED.length} runs`).toBe(
			EXPECTED.length
		);
		for (let i = 0; i < EXPECTED.length; i++) {
			const run = results.runs[i];
			expect(run.n).toBe(EXPECTED[i]);
			expect(
				run.received,
				`${contender} n=${run.n}: should receive at least ${EXPECTED[i]} events`
			).toBeGreaterThanOrEqual(EXPECTED[i]);
			expect(run.eventsPerSec, `${contender} n=${run.n}: throughput should be > 0`).toBeGreaterThan(
				0
			);
		}
	});
}

test.afterAll(() => {
	console.log('\n===== SUMMARY (all contenders) =====');
	console.log(JSON.stringify(allResults, null, 2));
});
