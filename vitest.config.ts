import { defineConfig } from 'vitest/config';
import { resolve } from 'path';

export default defineConfig({
	test: {
		globals: true,
		environment: 'node',
		include: ['src/**/*.test.ts'],
		testTimeout: 10000,
		hookTimeout: 10000,
		pool: 'forks', // Use forks to avoid WebSocket port conflicts
		coverage: {
			provider: 'v8',
			reporter: ['text', 'json', 'html'],
			exclude: ['node_modules/', 'dist/', '**/*.d.ts', '**/*.config.ts']
		}
	},
	resolve: {
		alias: {
			'src': resolve(__dirname, 'src')
		}
	}
});
