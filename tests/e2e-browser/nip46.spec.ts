import { test, expect } from '@playwright/test';
import { spawn, type ChildProcess } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const RELAY_PORT = 7746;

let relay: ChildProcess;

test.beforeAll(async () => {
	relay = spawn('node', [path.join(__dirname, 'mock-signer-relay.mjs'), '--port', String(RELAY_PORT)], {
		stdio: ['ignore', 'pipe', 'inherit']
	});
	await new Promise<void>((resolve, reject) => {
		const timeout = setTimeout(() => reject(new Error('mock signer relay did not start')), 10000);
		relay.stdout!.on('data', (d) => {
			process.stdout.write(d);
			if (d.toString().includes('listening')) {
				clearTimeout(timeout);
				resolve();
			}
		});
		relay.on('exit', (code) => {
			clearTimeout(timeout);
			reject(new Error(`mock signer relay exited early with code ${code}`));
		});
	});
});

test.afterAll(() => {
	relay?.kill();
});

test('NIP-46 bunker signing pipeline via mock signer relay', async ({ page }) => {
	test.setTimeout(60000);

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

	await page.goto('/tests/e2e-browser/nip46.html');

	await page.waitForFunction(() => {
		return (window as any).__testResults !== null;
	}, { timeout: 55000 });

	const results = await page.evaluate(() => (window as any).__testResults);

	console.log('NIP-46 E2E results:', JSON.stringify(results, null, 2));
	console.log('Browser logs:');
	logs.forEach(log => console.log(log));

	expect(results.pubkeySet, 'NIP-46 signer should be set').toBe(true);
	expect(results.activePubkey, 'active pubkey should be the mock signer pubkey').toBe(
		'6a04ab98d9e4774ad806e302dddeb63bea16b5cb5f223ee77478e861bb583eb3'
	);
	expect(results.signedEvent, 'signed event should be returned').toBeTruthy();
	expect(results.signatureValid, 'signed event should pass verifyEvent').toBe(true);
	expect(results.errors, 'no errors should occur').toHaveLength(0);
	expect(results.success, 'overall test should pass').toBe(true);
});
