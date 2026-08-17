import { defineConfig, Plugin } from 'vite';
import { resolve } from 'path';
import dts from 'vite-plugin-dts';
import wasm from 'vite-plugin-wasm';
import topLevelAwait from 'vite-plugin-top-level-await';

/**
 * Build specific entries as self-contained bundles (no shared chunks).
 * Uses esbuild to bundle all dependencies into a single file.
 */
function selfContainedEntries(entries: Array<{ input: string; output: string }>): Plugin {
	return {
		name: 'self-contained-entries',
		apply: 'build',
		enforce: 'post',
		async closeBundle() {
			const { build } = await import('esbuild');
			const fs = await import('fs');
			const path = await import('path');

			for (const entry of entries) {
				// Delete the Rollup-generated version if it exists
				const rollupOutput = path.resolve(process.cwd(), entry.output);
				if (fs.existsSync(rollupOutput)) {
					fs.unlinkSync(rollupOutput);
					const mapFile = rollupOutput + '.map';
					if (fs.existsSync(mapFile)) fs.unlinkSync(mapFile);
				}

				// Build self-contained version with esbuild
				await build({
					entryPoints: [path.resolve(process.cwd(), entry.input)],
					bundle: true,
					format: 'esm',
					platform: 'browser',
					target: 'es2022',
					outfile: path.resolve(process.cwd(), entry.output),
					external: [], // Bundle everything
					sourcemap: true,
					minify: true,
					loader: { '.ts': 'ts' }
				});

				console.log(`✓ ${entry.output} (self-contained)`);
			}
		}
	};
}

/** Fail the package build if a Node-only entry accidentally gains browser globals. */
function guardNodeEntries(entries: string[]): Plugin {
	return {
		name: 'guard-node-entries',
		apply: 'build',
		generateBundle(_options, bundle) {
			for (const entry of entries) {
				const output = bundle[entry];
				if (!output || output.type !== 'chunk') {
					throw new Error(`Missing Node package entry: ${entry}`);
				}
				if (/\b(?:document|window)\b/.test(output.code)) {
					throw new Error(`Browser global emitted in Node package entry: ${entry}`);
				}
			}
		}
	};
}

export default defineConfig({
	// Package assets must resolve relative to the importing worker module, not
	// from the consuming application's origin root.
	base: './',
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
		}),
		// Build proxy.ts as self-contained after main build
		selfContainedEntries([
			{ input: 'src/connections/proxy.ts', output: 'dist/connections/proxy.js' }
		]),
		guardNodeEntries(['proxy/server.js', 'proxy/vite.js', 'vite.js'])
	],
	resolve: {
		alias: {
			src: resolve(__dirname, 'src')
		}
	},
	build: {
		rollupOptions: {
			// App-mode defaults allow entry exports to be tree-shaken. These are
			// public package entry points, so preserve their module signatures.
			preserveEntrySignatures: 'strict',
			external: (id) => {
				return (
					['flatbuffers', 'nostr-tools', 'react-native', 'ws', 'socks-proxy-agent'].includes(id) ||
					id.startsWith('node:')
				);
			},
			input: {
				index: resolve(__dirname, 'src/index.ts'),
				utils: resolve(__dirname, 'src/utils.ts'),
				vite: resolve(__dirname, 'src/vite.ts'),
				hooks: resolve(__dirname, 'src/hooks.ts'),
				proxy: resolve(__dirname, 'src/proxy/index.ts'),
				proxyServer: resolve(__dirname, 'src/proxy/server.ts'),
				proxyVite: resolve(__dirname, 'src/proxy/vite.ts'),
				reactNative: resolve(__dirname, 'src/react-native.ts'),
				legacy: resolve(__dirname, 'src/legacy.ts'),
				connections: resolve(__dirname, 'src/connections/index.ts'),
				// proxy.ts is handled by selfContainedEntries
				cache: resolve(__dirname, 'src/cache/index.ts'),
				parser: resolve(__dirname, 'src/parser/index.ts'),
				crypto: resolve(__dirname, 'src/crypto/index.ts')
			},
			output: {
				format: 'es',
				entryFileNames: (chunkInfo: any) => {
					const entryNameMap: Record<string, string> = {
						index: 'index.js',
						utils: 'utils.js',
						vite: 'vite.js',
						hooks: 'hooks.js',
						proxy: 'proxy/index.js',
						proxyServer: 'proxy/server.js',
						proxyVite: 'proxy/vite.js',
						reactNative: 'react-native.js',
						legacy: 'legacy.js',
						connections: 'connections/index.js',
						cache: 'cache/index.js',
						parser: 'parser/index.js',
						crypto: 'crypto/index.js'
					};
					return entryNameMap[chunkInfo.name as string] || '[name].js';
				},
				chunkFileNames: '[name].js',
				assetFileNames: (assetInfo: any) => {
					if (assetInfo.name?.endsWith('.wasm')) {
						return 'wasm/[name][extname]';
					}
					return 'assets/[name][extname]';
				}
			}
		}
	},
	target: 'es2022',
	minify: 'esbuild',
	sourcemap: true,
	assetsInlineLimit: 0
});
