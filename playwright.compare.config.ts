import { defineConfig, devices } from '@playwright/test';

// Head-to-head contender comparison suite. Shares the vite dev server on 5375
// with the bench suite but uses its own mock relay on 7711 so both suites can
// run side by side. Chromium flags enable window.gc() and precise
// performance.memory for honest JS-heap measurements.
export default defineConfig({
	testDir: './tests/compare',
	fullyParallel: false,
	forbidOnly: !!process.env.CI,
	retries: 0,
	workers: 1,
	reporter: 'list',
	use: {
		baseURL: 'http://localhost:5375',
		trace: 'on-first-retry',
		launchOptions: {
			args: ['--js-flags=--expose-gc', '--enable-precise-memory-info']
		}
	},
	projects: [
		{
			name: 'chromium',
			use: { ...devices['Desktop Chrome'] }
		}
	],
	webServer: [
		{
			command: 'node tests/bench/mock-relay.mjs --port 7711 --live 0',
			port: 7711,
			reuseExistingServer: !process.env.CI,
			timeout: 30000
		},
		{
			command: 'npx vite --config tests/bench/vite.bench.config.mjs --port 5375 --strictPort',
			// The repo root has no index.html (vite 404s `/`, which playwright
			// does not accept), so probe the compare page directly.
			url: 'http://localhost:5375/tests/compare/compare.html',
			reuseExistingServer: !process.env.CI,
			timeout: 120000
		}
	]
});
