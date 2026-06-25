import { test, expect } from '@playwright/test';

test('NIP-07 signing pipeline via engine worker', async ({ page }) => {
	test.setTimeout(30000);

	const logs: string[] = [];
	page.on('console', (msg) => {
		logs.push(`[${msg.type()}] ${msg.text()}`);
	});
	page.on('pageerror', (err) => {
		logs.push(`[ERROR] ${err.message}`);
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

	await page.goto('/tests/e2e-browser/nip07.html');

	await page.waitForFunction(() => {
		return (window as any).__testResults !== null;
	}, { timeout: 25000 });

	const results = await page.evaluate(() => (window as any).__testResults);

	console.log('NIP-07 E2E results:', JSON.stringify(results, null, 2));
	console.log('Browser logs:');
	logs.forEach(log => console.log(log));

	expect(results.pubkeySet, 'NIP-07 pubkey should be set').toBe(true);
	expect(results.activePubkey, 'active pubkey should match mock').toBeTruthy();
	expect(results.getPublicKeyCalled, 'window.nostr.getPublicKey should be called').toBe(true);
	expect(results.signEventCalled, 'window.nostr.signEvent should be called').toBe(true);
	expect(results.signedEvent, 'signed event should be returned').toBeTruthy();
	expect(results.errors, 'no errors should occur').toHaveLength(0);
	expect(results.success, 'overall test should pass').toBe(true);
});
