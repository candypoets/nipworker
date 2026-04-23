import { test, expect } from '@playwright/test';

test('debug worker wrapper - all network', async ({ page }) => {
	test.setTimeout(45000);

	page.on('response', (response) => {
		const url = response.url();
		if (url.includes('pkg') || url.includes('.wasm') || url.includes('index.js') || url.includes('index.ts')) {
			console.log(`[NETWORK ${response.status()}] ${url}`);
		}
	});
	page.on('console', (msg) => {
		console.log(`[console:${msg.type()}] ${msg.text()}`);
	});
	page.on('worker', (worker) => {
		console.log(`[worker] ${worker.url()}`);
		worker.on('console', (msg) => {
			const text = msg.text();
			if (text.includes('INFO') || text.includes('WARN') || text.includes('ERROR') || text.includes('loop') || text.includes('started')) {
				console.log(`[W:${worker.url().split('/').pop()?.slice(0,10)}] ${text.substring(0, 120)}`);
			}
		});
	});

	await page.goto('/tests/e2e-browser/worker-wrapper.html');
	await page.waitForTimeout(25000);

	const results = await page.evaluate(() => (window as any).__testResults);
	console.log('Results:', JSON.stringify(results, null, 2));
});
