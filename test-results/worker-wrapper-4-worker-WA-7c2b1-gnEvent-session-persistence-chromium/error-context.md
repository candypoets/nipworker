# Instructions

- Following Playwright test failed.
- Explain why, be concise, respect Playwright best practices.
- Provide a snippet of code with the fix, if possible.

# Test info

- Name: worker-wrapper.spec.ts >> 4-worker WASM: subscribe, signer, signEvent, session persistence
- Location: tests/e2e-browser/worker-wrapper.spec.ts:3:1

# Error details

```
Test timeout of 90000ms exceeded.
```

```
Error: page.waitForFunction: Test timeout of 90000ms exceeded.
```

# Page snapshot

```yaml
- generic [active] [ref=e1]:
  - heading "4-Worker WASM E2E Test" [level=1] [ref=e2]
  - generic [ref=e3]: ❌ EXCEPTION
  - generic [ref=e4]: "✓ NostrManager (4-worker) created === Test 1: Basic Subscribe === sub1: 0 messages, 0 events, EOCE=false === Test 2: Privkey Signer + getPublicKey ==="
```

# Test source

```ts
  1  | import { test, expect } from '@playwright/test';
  2  | 
  3  | test('4-worker WASM: subscribe, signer, signEvent, session persistence', async ({ page }) => {
  4  | 	// Overall timeout: subscribe (30s) + signer setup + signEvent + reload + restore
  5  | 	test.setTimeout(90000);
  6  | 
  7  | 	const logs: string[] = [];
  8  | 	page.on('console', (msg) => {
  9  | 		logs.push(`[${msg.type()}] ${msg.text()}`);
  10 | 	});
  11 | 	page.on('pageerror', (err) => {
  12 | 		logs.push(`[ERROR] ${err.message}`);
  13 | 	});
  14 | 	page.on('worker', (worker) => {
  15 | 		logs.push(`[worker] Worker created: ${worker.url()}`);
  16 | 		worker.on('console', (msg) => {
  17 | 			logs.push(`[worker:${msg.type()}] ${msg.text()}`);
  18 | 		});
  19 | 		worker.on('pageerror', (err) => {
  20 | 			logs.push(`[worker:ERROR] ${err.message}`);
  21 | 		});
  22 | 	});
  23 | 
  24 | 	// ---- Phase 1: Initial load ----
  25 | 	await page.goto('/tests/e2e-browser/worker-wrapper.html');
  26 | 
  27 | 	// Wait for phase 1 to complete
> 28 | 	await page.waitForFunction(() => {
     |             ^ Error: page.waitForFunction: Test timeout of 90000ms exceeded.
  29 | 		return (window as any).__testPhase1Complete === true;
  30 | 	}, { timeout: 60000 });
  31 | 
  32 | 	const phase1Results = await page.evaluate(() => (window as any).__testResults);
  33 | 
  34 | 	console.log('Phase 1 results:', JSON.stringify(phase1Results, null, 2));
  35 | 
  36 | 	// Assertions for phase 1
  37 | 	expect(phase1Results.subscribeWorks, 'subscribe should receive at least one event').toBe(true);
  38 | 	expect(phase1Results.sub1Events, 'sub1 should receive at least one event').toBeGreaterThan(0);
  39 | 
  40 | 	expect(phase1Results.signerSet, 'privkey signer should be set').toBe(true);
  41 | 	expect(phase1Results.signerPubkey, 'signer pubkey should be set').toBeTruthy();
  42 | 
  43 | 	expect(phase1Results.getPublicKeyWorks, 'getPublicKey should return consistent pubkey').toBe(true);
  44 | 	expect(phase1Results.getPublicKeyResult, 'getPublicKey result should match signer pubkey').toBe(phase1Results.signerPubkey);
  45 | 
  46 | 	expect(phase1Results.signEventWorks, 'signEvent should return event with matching pubkey').toBe(true);
  47 | 	expect(phase1Results.signedEvent, 'signed event should exist').toBeTruthy();
  48 | 	expect(phase1Results.signedEvent.pubkey, 'signed event pubkey should match').toBe(phase1Results.signerPubkey);
  49 | 
  50 | 	expect(phase1Results.sessionPersisted, 'session should be persisted to localStorage').toBe(true);
  51 | 
  52 | 	// ---- Phase 2: Reload and verify session restore ----
  53 | 	await page.goto('/tests/e2e-browser/worker-wrapper.html?reload=1');
  54 | 
  55 | 	await page.waitForFunction(() => {
  56 | 		return (window as any).__testResults !== null;
  57 | 	}, { timeout: 20000 });
  58 | 
  59 | 	const phase2Results = await page.evaluate(() => (window as any).__testResults);
  60 | 
  61 | 	console.log('Phase 2 results:', JSON.stringify(phase2Results, null, 2));
  62 | 	console.log('Browser logs:');
  63 | 	logs.forEach(log => console.log(log));
  64 | 
  65 | 	expect(phase2Results.sessionRestoredAfterReload, 'session should restore after reload').toBe(true);
  66 | 	expect(phase2Results.restoredPubkey, 'restored pubkey should match original').toBe(phase1Results.signerPubkey);
  67 | 
  68 | 	expect(phase2Results.success, 'overall test should pass').toBe(true);
  69 | });
  70 | 
```