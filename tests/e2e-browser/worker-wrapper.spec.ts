import { test, expect } from '@playwright/test';

test('4-worker WASM: subscribe, signer, signEvent, session persistence', async ({ page }) => {
	// Overall timeout: subscribe (30s) + signer setup + signEvent + reload + restore
	test.setTimeout(90000);

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

	// ---- Phase 1: Initial load ----
	await page.goto('/tests/e2e-browser/worker-wrapper.html');

	// Wait for phase 1 to complete
	await page.waitForFunction(() => {
		return (window as any).__testPhase1Complete === true;
	}, { timeout: 60000 });

	const phase1Results = await page.evaluate(() => (window as any).__testResults);

	console.log('Phase 1 results:', JSON.stringify(phase1Results, null, 2));

	// Assertions for phase 1
	expect(phase1Results.subscribeWorks, 'subscribe should receive at least one event').toBe(true);
	expect(phase1Results.sub1Events, 'sub1 should receive at least one event').toBeGreaterThan(0);

	expect(phase1Results.signerSet, 'privkey signer should be set').toBe(true);
	expect(phase1Results.signerPubkey, 'signer pubkey should be set').toBeTruthy();

	expect(phase1Results.getPublicKeyWorks, 'getPublicKey should return consistent pubkey').toBe(true);
	expect(phase1Results.getPublicKeyResult, 'getPublicKey result should match signer pubkey').toBe(phase1Results.signerPubkey);

	expect(phase1Results.signEventWorks, 'signEvent should return event with matching pubkey').toBe(true);
	expect(phase1Results.signedEvent, 'signed event should exist').toBeTruthy();
	expect(phase1Results.signedEvent.pubkey, 'signed event pubkey should match').toBe(phase1Results.signerPubkey);

	expect(phase1Results.sessionPersisted, 'session should be persisted to localStorage').toBe(true);

	// ---- Phase 2: Reload and verify session restore ----
	await page.goto('/tests/e2e-browser/worker-wrapper.html?reload=1');

	await page.waitForFunction(() => {
		return (window as any).__testResults !== null;
	}, { timeout: 20000 });

	const phase2Results = await page.evaluate(() => (window as any).__testResults);

	console.log('Phase 2 results:', JSON.stringify(phase2Results, null, 2));
	console.log('Browser logs:');
	logs.forEach(log => console.log(log));

	expect(phase2Results.sessionRestoredAfterReload, 'session should restore after reload').toBe(true);
	expect(phase2Results.restoredPubkey, 'restored pubkey should match original').toBe(phase1Results.signerPubkey);

	expect(phase2Results.success, 'overall test should pass').toBe(true);
});
