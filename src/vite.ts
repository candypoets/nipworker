import { Buffer } from 'node:buffer';
import { readFileSync } from 'node:fs';
import { dirname, posix, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import type { Plugin } from 'vite';

const wasmFiles = [
	'nipworker_connections_bg.wasm',
	'nipworker_cache_bg.wasm',
	'nipworker_parser_bg.wasm',
	'nipworker_crypto_bg.wasm'
] as const;

function javascriptSource(output: {
	fileName: string;
	type: 'asset' | 'chunk';
	code?: string;
	source?: string | Uint8Array;
}): string | undefined {
	if (!/\.[cm]?js$/.test(output.fileName)) return undefined;
	if (output.type === 'chunk') return output.code;
	if (typeof output.source === 'string') return output.source;
	if (output.source instanceof Uint8Array) return Buffer.from(output.source).toString('utf8');
	return undefined;
}

/**
 * Emit the WASM binaries referenced by nipworker's published worker modules.
 *
 * Vite treats the already-built worker modules as assets when it consumes the
 * package, so it does not discover their relative WASM URLs itself. This plugin
 * finds those emitted workers and places each binary at the URL it references.
 */
export function nipworkerWasmPlugin(): Plugin {
	const wasmDirectory = resolve(dirname(fileURLToPath(import.meta.url)), 'wasm');

	return {
		name: 'nipworker-wasm',
		apply: 'build',
		generateBundle(_options, bundle) {
			const bundledWasm = new Set(
				Object.values(bundle)
					.map((output) => output.fileName)
					.filter((fileName) => fileName.endsWith('.wasm'))
					.map((fileName) =>
						wasmFiles.find((file) => fileName.includes(file.slice(0, -'.wasm'.length)))
					)
					.filter((file): file is (typeof wasmFiles)[number] => file !== undefined)
			);

			// New package builds expose the WASM URLs from the main module, allowing
			// Vite to emit them normally. Keep this fallback for older consumers while
			// avoiding a second, unhashed copy of every binary.
			if (bundledWasm.size === wasmFiles.length) return;

			const targets = new Map<string, (typeof wasmFiles)[number]>();

			for (const output of Object.values(bundle)) {
				const source = javascriptSource(output);
				if (!source) continue;

				for (const file of wasmFiles) {
					const reference = `../wasm/${file}`;
					if (!source.includes(reference)) continue;

					const target = posix.normalize(posix.join(posix.dirname(output.fileName), reference));
					if (target.startsWith('../')) {
						this.error(`Cannot emit ${file} outside Vite's output directory`);
					}
					targets.set(target, file);
				}
			}

			for (const [fileName, sourceFile] of targets) {
				if (bundle[fileName]) continue;
				this.emitFile({
					type: 'asset',
					fileName,
					source: readFileSync(resolve(wasmDirectory, sourceFile))
				});
			}
		}
	};
}
