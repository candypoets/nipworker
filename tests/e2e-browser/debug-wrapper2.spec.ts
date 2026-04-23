import { test, expect } from '@playwright/test';

test('debug worker wrapper with worker logs', async ({ page }) => {
	test.setTimeout(45000);

	page.on('console', (msg) => {
		console.log(`[console:${msg.type()}] ${msg.text()}`);
	});
	page.on('pageerror', (err) => {
		console.log(`[pageerror] ${err.message}`);
	});
	page.on('worker', (worker) => {
		console.log(`[worker] ${worker.url()}`);
		worker.on('console', (msg) => {
			console.log(`[worker:${worker.url().split('/').pop()}:${msg.type()}] ${msg.text()}`);
		});
		worker.on('pageerror', (err) => {
			console.log(`[worker:error] ${err.message}`);
		});
	});

	await page.goto('/tests/e2e-browser/worker-wrapper.html');
	await page.waitForTimeout(25000);

	const results = await page.evaluate(() => (window as any).__testResults);
	console.log('Results:', JSON.stringify(results, null, 2));
});
