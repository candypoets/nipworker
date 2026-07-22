import { test, expect } from '@playwright/test';

// Runs the built bundle over a non-localhost HTTP origin (`nipworker.test`
// mapped to 127.0.0.1). Non-localhost HTTP is not a secure context, so
// `navigator.storage` / OPFS is genuinely unavailable and the cache worker
// must degrade to in-memory storage without uncaught errors.
test.use({
	launchOptions: {
		args: ['--host-resolver-rules=MAP nipworker.test 127.0.0.1'],
	},
});

test('cache worker degrades gracefully when OPFS is unavailable (insecure context)', async ({
	page,
}) => {
	test.setTimeout(60000);

	const logs: string[] = [];
	page.on('console', (msg) => {
		logs.push(`[${msg.type()}] ${msg.text()}`);
	});
	page.on('pageerror', (err) => {
		logs.push(`[PAGEERROR] ${err.message}`);
	});

	await page.goto('http://nipworker.test:5417/tests/e2e-browser/no-opfs.html');

	// Precondition: this origin really is an insecure context without OPFS.
	const opfsAvailable = await page.evaluate(
		() => typeof navigator.storage?.getDirectory === 'function'
	);
	expect(opfsAvailable, 'OPFS must be unavailable on this origin').toBe(false);

	await page.waitForFunction(() => (window as any).__testResults != null, { timeout: 45000 });

	const results = await page.evaluate(() => (window as any).__testResults);

	console.log('E2E results:', JSON.stringify(results, null, 2));
	console.log('Browser logs:');
	logs.forEach((log) => console.log(log));

	// The original bug: uncaught TypeError from getDirectory crossing the
	// JS/WASM boundary. No uncaught errors may surface from any worker.
	const uncaught = logs.filter((l) => l.includes('Uncaught'));
	expect(uncaught, 'no uncaught worker errors').toHaveLength(0);
	expect(results.errors).toHaveLength(0);

	// The app must still work end-to-end on the (empty) in-memory cache:
	// network events flow, and a cache-only query terminates with EOCE.
	expect(results.networkEvents, 'should receive network events').toBeGreaterThan(0);
	expect(results.eoce, 'cache-only query must receive EOCE').toBe(true);
	expect(results.ok, 'overall success').toBe(true);
});
