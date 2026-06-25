import { test, expect } from '@playwright/test';

test('4-worker WASM: cache persists events and returns them before EOCE', async ({ page }) => {
	test.setTimeout(90000);

	const logs: string[] = [];
	page.on('console', (msg) => {
		logs.push(`[${msg.type()}] ${msg.text()}`);
	});
	page.on('pageerror', (err) => {
		logs.push(`[ERROR] ${err.message}`);
	});

	await page.goto('/tests/e2e-browser/worker-cache.html');

	await page.waitForFunction(() => {
		return (window as any).__testResults !== null;
	}, { timeout: 80000 });

	const results = await page.evaluate(() => (window as any).__testResults);

	console.log('E2E results:', JSON.stringify(results, null, 2));
	console.log('Browser logs:');
	logs.forEach(log => console.log(log));

	// Sub1 must have received network events
	const sub1Events = results.sub1.filter((m: any) => m.type === 'ParsedNostrEvent');
	expect(sub1Events.length, 'sub1 should receive at least one event').toBeGreaterThan(0);
	expect(results.errors).toHaveLength(0);

	// All cache-only filter tests must pass
	expect(results.cacheTests, 'cacheTests array should exist').toBeDefined();
	expect(results.cacheTests.length, 'should have cache test results').toBeGreaterThan(0);

	const passedCacheTests = results.cacheTests.filter((t: any) => t.passed);
	const failedCacheTests = results.cacheTests.filter((t: any) => !t.passed);

	console.log(`\nCache filter tests: ${passedCacheTests.length}/${results.cacheTests.length} passed`);

	if (failedCacheTests.length > 0) {
		console.log('\nFailed cache tests:');
		failedCacheTests.forEach((t: any) => {
			console.log(`  ✗ ${t.name}: ${t.events} events - ${t.error}`);
		});
	}

	expect(failedCacheTests.length, 'all cache filter tests should pass').toBe(0);

	// Sub2 must have received cached events BEFORE EOCE
	const sub2Events = results.sub2.filter((m: any) => m.type === 'ParsedNostrEvent');

	const parsedIdx = results.sub2.findIndex((m: any) => m.type === 'ParsedNostrEvent');
	const eoceIdx = results.sub2.findIndex((m: any) => m.type === 'Eoce');

	expect(parsedIdx, 'sub2 should receive at least one ParsedNostrEvent').toBeGreaterThanOrEqual(0);
	expect(eoceIdx, 'sub2 should receive an EOCE').toBeGreaterThanOrEqual(0);
	expect(parsedIdx, 'ParsedNostrEvent must arrive before EOCE').toBeLessThan(eoceIdx);

	// Verify cache hit flag is set
	expect(results.cacheHit, 'cacheHit should be true').toBe(true);

	// Verify overall success
	expect(results.success, 'overall test success').toBe(true);
});
