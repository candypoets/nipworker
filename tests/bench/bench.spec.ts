import { test, expect } from '@playwright/test';

test('bench: throughput, cache query latency, e2e latency', async ({ page }) => {
	test.setTimeout(240000);

	const logs: string[] = [];
	page.on('console', (msg) => {
		logs.push(`[${msg.type()}] ${msg.text()}`);
	});
	page.on('pageerror', (err) => {
		logs.push(`[ERROR] ${err.message}`);
	});

	await page.goto('/tests/bench/bench.html');

	await page.waitForFunction(
		() => {
			return (window as any).__benchResults !== null && (window as any).__benchResults !== undefined;
		},
		{ timeout: 220000 }
	);

	const results = await page.evaluate(() => (window as any).__benchResults);

	console.log('Bench results:', JSON.stringify(results, null, 2));
	console.log('Browser logs:');
	logs.forEach((log) => console.log(log));

	// Loose sanity assertions: this is a measurement harness, not a pass/fail gate.
	expect(results.errors, 'bench errors should be empty').toHaveLength(0);

	expect(results.throughput.length, 'should have throughput runs').toBeGreaterThan(0);
	for (const run of results.throughput) {
		expect(run.eventsPerSec, `throughput for n=${run.events} should be > 0`).toBeGreaterThan(0);
		expect(run.received, `should have received events for n=${run.events}`).toBeGreaterThan(0);
	}

	expect(results.cacheQueryLatency.length, 'should have cache latency stats').toBeGreaterThan(0);
	for (const q of results.cacheQueryLatency) {
		expect(q.samples.length, `cache limit=${q.limit} should have samples`).toBeGreaterThan(0);
	}

	expect(results.e2eLatency, 'should have e2e latency results').not.toBeNull();
	expect(
		results.e2eLatency.reqToFirstEventMs,
		'req->first-event latency should be > 0'
	).toBeGreaterThan(0);
});
