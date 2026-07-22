import { defineConfig, devices } from '@playwright/test';

export default defineConfig({
	testDir: './tests/bench',
	fullyParallel: false,
	forbidOnly: !!process.env.CI,
	retries: 0,
	workers: 1,
	reporter: 'list',
	use: {
		baseURL: 'http://localhost:5375',
		trace: 'on-first-retry'
	},
	projects: [
		{
			name: 'chromium',
			use: { ...devices['Desktop Chrome'] }
		}
	],
	webServer: [
		{
			command:
				'node tests/bench/mock-relay.mjs --port 7710 --live 200 --live-delay 1000 --live-prefix bench_e2e',
			port: 7710,
			reuseExistingServer: !process.env.CI,
			timeout: 30000
		},
		{
			command: 'npx vite --config tests/bench/vite.bench.config.mjs --port 5375 --strictPort',
			// The repo root has no index.html (vite 404s `/`, which playwright
			// does not accept), so probe the bench page directly.
			url: 'http://localhost:5375/tests/bench/bench.html',
			reuseExistingServer: !process.env.CI,
			timeout: 120000
		}
	]
});
