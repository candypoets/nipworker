import { defineConfig } from 'vite';
import { resolve } from 'path';
import dts from 'vite-plugin-dts';
import wasm from 'vite-plugin-wasm';
import topLevelAwait from 'vite-plugin-top-level-await';

export default defineConfig({
	plugins: [
		wasm(),
		topLevelAwait(),
		dts({
			include: ['src/**/*'],
			exclude: ['src/**/*.test.*', 'src/**/*.spec.*'],
			outDir: 'dist',
			insertTypesEntry: true,
			entryRoot: 'src',
			rollupTypes: false,
			copyDtsFiles: true,
			pathsToAliases: false
		})
	],
	resolve: {
		alias: {
			src: resolve(__dirname, 'src')
		}
	},
	build: {
		lib: {
			entry: resolve(__dirname, 'src/index.ts'),
			name: 'NipWorker',
			formats: ['es'],
			fileName: 'index.js'
		},
		rollupOptions: {
			external: (id) => {
				return ['flatbuffers', 'nostr-tools'].includes(id);
			},
			input: {
				index: resolve(__dirname, 'src/index.ts'),
				// types: resolve(__dirname, "src/types/index.ts"),
				utils: resolve(__dirname, 'src/utils.ts'),
				hooks: resolve(__dirname, 'src/hooks.ts'),
				connections: resolve(__dirname, 'src/connections/index.ts'),
				cache: resolve(__dirname, 'src/cache/index.ts'),
				parser: resolve(__dirname, 'src/parser/index.ts'),
				crypto: resolve(__dirname, 'src/crypto/index.ts')
			},
			output: {
				entryFileNames: (chunkInfo) => {
					// Map entry names to desired output filenames
					const entryNameMap: Record<string, string> = {
						index: 'index.js',
						utils: 'utils.js',
						hooks: 'hooks.js',
						connections: 'connections/index.js',
						cache: 'cache/index.js',
						parser: 'parser/index.js',
						crypto: 'crypto/index.js'
					};
					return entryNameMap[chunkInfo.name as string] || '[name].js';
				},
				// Handle .d.ts files
				chunkFileNames: (chunkInfo) => {
					return '[name].js';
				},
				// Ensure worker and WASM files are properly handled
				assetFileNames: (assetInfo) => {
					if (assetInfo.name?.endsWith('.wasm')) {
						return 'wasm/[name][extname]';
					}
					if (assetInfo.name?.includes('worker')) {
						return 'workers/[name][extname]';
					}
					return 'assets/[name][extname]';
				}
			}
		},
		target: 'es2022',
		minify: 'esbuild',
		sourcemap: true,
		// Prevent inlining of assets - this is key!
		assetsInlineLimit: 0 // This prevents base64 inlining
	}
});
