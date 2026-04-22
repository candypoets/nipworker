import { defineConfig, devices } from '@playwright/test';

export default defineConfig({
	testDir: './tests/e2e-browser',
	fullyParallel: false,
	forbidOnly: !!process.env.CI,
	retries: process.env.CI ? 2 : 0,
	workers: 1,
	reporter: 'list',
	use: {
		baseURL: 'http://localhost:5174',
		trace: 'on-first-retry',
	},
	projects: [
		{
			name: 'chromium',
			use: { ...devices['Desktop Chrome'] },
		},
	],
	// Run `npx vite --port 5173` manually before executing tests.
	// webServer: {
	// 	command: 'npx vite --port 5173',
	// 	url: 'http://localhost:5174',
	// 	reuseExistingServer: !process.env.CI,
	// 	timeout: 120000,
	// },
});
