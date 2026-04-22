import { test, expect } from '@playwright/test';

test('debug engine worker', async ({ page }) => {
	test.setTimeout(30000);

	const logs: string[] = [];
	page.on('console', (msg) => {
		logs.push(`[main:${msg.type()}] ${msg.text()}`);
	});
	page.on('pageerror', (err) => {
		logs.push(`[main:ERROR] ${err.message}`);
	});
	page.on('worker', (worker) => {
		logs.push(`[worker] Worker created: ${worker.url()}`);
		worker.on('console', (msg) => {
			logs.push(`[worker:${msg.type()}] ${msg.text()}`);
		});
		worker.on('pageerror', (err) => {
			logs.push(`[worker:ERROR] ${err.message}`);
		});
	});

	await page.goto('http://localhost:5174/tests/e2e-browser/index.html');
	await page.waitForTimeout(5000);

	const results = await page.evaluate(() => (window as any).__testResults);
	console.log('Results:', JSON.stringify(results, null, 2));
	console.log('Logs:');
	logs.forEach((l) => console.log(l));
});
