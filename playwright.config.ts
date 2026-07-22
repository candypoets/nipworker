import { defineConfig, devices } from '@playwright/test';

export default defineConfig({
	testDir: './tests/e2e-browser',
	fullyParallel: false,
	forbidOnly: !!process.env.CI,
	retries: process.env.CI ? 2 : 0,
	workers: 1,
	reporter: 'list',
	use: {
		baseURL: process.env.PW_BASE_URL ?? 'http://localhost:5174',
		trace: 'on-first-retry',
	},
	projects: [
		{
			name: 'chromium',
			use: { ...devices['Desktop Chrome'] },
		},
	],
	// Run `npx vite --config tests/e2e-browser/vite.e2e.config.ts --port 5174` manually before executing tests.
	// (The e2e config rewrites the workers' `/src/*/index.js` URLs to their `.ts` sources;
	// a plain `npx vite` serves 404s for them and the workers never boot.)
	// webServer: {
	// 	command: 'npx vite --port 5173',
	// 	url: 'http://localhost:5174',
	// 	reuseExistingServer: !process.env.CI,
	// 	timeout: 120000,
	// },
});
