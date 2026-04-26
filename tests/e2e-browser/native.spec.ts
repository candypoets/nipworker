import { test, expect } from '@playwright/test';

test('NativeBackend: signer, signEvent, session persistence, logout', async ({ page }) => {
	test.setTimeout(30000);

	const logs: string[] = [];
	page.on('console', (msg) => {
		logs.push(`[${msg.type()}] ${msg.text()}`);
	});
	page.on('pageerror', (err) => {
		logs.push(`[ERROR] ${err.message}`);
	});

	await page.goto('/tests/e2e-browser/native.html');

	// Wait for the test to complete and expose results on window
	await page.waitForFunction(() => {
		return (window as any).__testResults !== null;
	}, { timeout: 25000 });

	const results = await page.evaluate(() => (window as any).__testResults);

	console.log('Native E2E results:', JSON.stringify(results, null, 2));
	console.log('Browser logs:');
	logs.forEach(log => console.log(log));

	// Assertions
	expect(results.backendCreated, 'NativeBackend should be created').toBe(true);
	expect(results.signerSet, 'privkey signer should be set').toBe(true);
	expect(results.signerPubkey, 'signer pubkey should be set').toBeTruthy();

	expect(results.getPublicKeyWorks, 'getPublicKey should return expected pubkey').toBe(true);
	expect(results.getPublicKeyResult, 'getPublicKey result should match expected').toBe('79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798');

	expect(results.signEventWorks, 'signEvent should return event with matching pubkey').toBe(true);
	expect(results.signedEvent, 'signedEvent should exist').toBeTruthy();
	expect(results.signedEvent.pubkey, 'signedEvent pubkey should match').toBe('79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798');
	expect(results.signedEvent.kind, 'signedEvent kind should match').toBe(1);
	expect(results.signedEvent.content, 'signedEvent content should match').toBe('Hello from NativeBackend E2E test');
	expect(results.signedEvent.sig, 'signedEvent sig should be set').toBeTruthy();
	expect(results.signedEvent.id, 'signedEvent id should be set').toBeTruthy();

	expect(results.sessionPersisted, 'session should be persisted to localStorage').toBe(true);
	expect(results.logoutWorks, 'logout should clear active pubkey').toBe(true);
	expect(results.activePubkeyAfterLogout, 'activePubkey after logout should be null').toBeNull();

	expect(results.errors).toHaveLength(0);
	expect(results.success, 'overall test success').toBe(true);
});
