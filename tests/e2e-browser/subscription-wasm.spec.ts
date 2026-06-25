import { test, expect } from '@playwright/test';

test('useSubscription via EngineManager receives events and EOCE', async ({ page }) => {
	test.setTimeout(30000);

	const logs: string[] = [];
	page.on('console', (msg) => {
		logs.push(`[${msg.type()}] ${msg.text()}`);
	});
	page.on('pageerror', (err) => {
		logs.push(`[ERROR] ${err.message}`);
	});
	page.on('worker', (worker) => {
		logs.push(`[worker] ${worker.url()}`);
		worker.on('console', (msg) => {
			logs.push(`[worker:${msg.type()}] ${msg.text()}`);
		});
		worker.on('pageerror', (err) => {
			logs.push(`[worker:ERROR] ${err.message}`);
		});
	});

	await page.goto('/tests/e2e-browser/subscription-wasm.html');

	await page.waitForFunction(() => {
		return (window as any).__testResults !== null;
	}, { timeout: 25000 });

	const results = await page.evaluate(() => (window as any).__testResults);

	console.log('WASM Subscription E2E results:', JSON.stringify(results, null, 2));
	console.log('Browser logs:');
	logs.forEach(log => console.log(log));

	expect(results.eventsReceived, 'should receive at least one event').toBeGreaterThan(0);
	expect(results.eoceReceived, 'should receive EOCE').toBe(true);
	expect(results.errors).toHaveLength(0);
	expect(results.success, 'overall test success').toBe(true);
});
