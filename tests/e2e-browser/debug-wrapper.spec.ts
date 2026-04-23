import { test, expect } from '@playwright/test';

test('debug worker wrapper', async ({ page }) => {
	test.setTimeout(30000);

	page.on('response', (response) => {
		if (response.status() >= 400) {
			console.log(`[NETWORK ${response.status()}] ${response.url()}`);
		}
	});

	page.on('console', (msg) => {
		console.log(`[console:${msg.type()}] ${msg.text()}`);
	});
	page.on('pageerror', (err) => {
		console.log(`[pageerror] ${err.message}`);
	});
	page.on('worker', (worker) => {
		console.log(`[worker] ${worker.url()}`);
		worker.on('console', (msg) => console.log(`[worker:${msg.type()}] ${msg.text()}`));
		worker.on('pageerror', (err) => console.log(`[worker:error] ${err.message}`));
	});

	await page.goto('/tests/e2e-browser/worker-wrapper.html');
	await page.waitForTimeout(15000);
});
