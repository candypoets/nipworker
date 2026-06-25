import { test, expect } from '@playwright/test';

test('debug engine worker URLs', async ({ page }) => {
	test.setTimeout(30000);

	page.on('response', (response) => {
		if (response.url().includes('engine') || response.url().includes('worker')) {
			console.log(`[NETWORK ${response.status()}] ${response.url()}`);
		}
	});
	page.on('worker', (worker) => {
		console.log(`[worker] ${worker.url()}`);
	});
	page.on('console', (msg) => {
		if (msg.text().includes('Worker') || msg.text().includes('engine')) {
			console.log(`[console] ${msg.text()}`);
		}
	});

	await page.goto('/tests/e2e-browser/index.html');
	await page.waitForTimeout(10000);
});
