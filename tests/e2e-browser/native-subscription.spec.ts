import { test, expect } from '@playwright/test';

test('useSubscription with NativeBackend receives events and EOSE', async ({ page }) => {
	test.setTimeout(30000);

	const logs: string[] = [];
	page.on('console', (msg) => {
		logs.push(`[${msg.type()}] ${msg.text()}`);
	});
	page.on('pageerror', (err) => {
		logs.push(`[ERROR] ${err.message}`);
	});

	await page.goto('/tests/e2e-browser/native-subscription.html');

	// Wait for the test to complete (up to 10 seconds)
	await page.waitForFunction(
		() => (window as any).__testResult !== undefined,
		undefined,
		{ timeout: 10000 }
	);

	const result = await page.evaluate(() => (window as any).__testResult);

	console.log('Native subscription E2E results:', JSON.stringify(result, null, 2));
	console.log('Browser logs:');
	logs.forEach(log => console.log(log));

	expect(result.pubkey).toBe('79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798');
	expect(result.eose).toBe(true);
	expect(result.events.length).toBe(3);

	for (let i = 0; i < 3; i++) {
		const event = result.events[i];
		expect(event.kind).toBe(1);
		expect(event.pubkey).toBe('79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798');
	}

	// Verify DOM state
	const eventItems = page.locator('li.event-item');
	await expect(eventItems).toHaveCount(3);
	await expect(page.locator('#eose')).toHaveText('EOSE received, eventCount=3');
});
