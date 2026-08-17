import assert from 'node:assert/strict';
import { mkdtemp, mkdir, readdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

import { build } from 'vite';

const wasmNames = [
	'nipworker_connections_bg',
	'nipworker_cache_bg',
	'nipworker_parser_bg',
	'nipworker_crypto_bg'
];

async function filesUnder(directory, prefix = '') {
	const files = [];
	for (const entry of await readdir(directory, { withFileTypes: true })) {
		const relative = join(prefix, entry.name);
		if (entry.isDirectory()) {
			files.push(...(await filesUnder(join(directory, entry.name), relative)));
		} else {
			files.push(relative);
		}
	}
	return files;
}

const fixtureRoot = await mkdtemp(join(tmpdir(), 'nipworker-vite-consumer-'));
const outputDirectory = join(fixtureRoot, 'build');

try {
	await mkdir(join(fixtureRoot, 'src'));
	await writeFile(
		join(fixtureRoot, 'index.html'),
		'<!doctype html><script type="module" src="/src/main.js"></script>'
	);
	await writeFile(
		join(fixtureRoot, 'src/main.js'),
		"import { createNostrManager } from '@candypoets/nipworker';\n" +
			'window.createNostrManager = createNostrManager;\n'
	);

	await build({
		configFile: false,
		root: fixtureRoot,
		logLevel: 'silent',
		resolve: {
			alias: {
				'@candypoets/nipworker': resolve('dist/index.js')
			}
		},
		build: {
			outDir: outputDirectory,
			emptyOutDir: true,
			assetsInlineLimit: 0
		}
	});

	const outputFiles = await filesUnder(outputDirectory);
	const wasmFiles = outputFiles.filter((file) => file.endsWith('.wasm'));
	assert.equal(
		wasmFiles.length,
		wasmNames.length,
		`Unexpected WASM output: ${wasmFiles.join(', ')}`
	);
	for (const name of wasmNames) {
		assert.ok(
			wasmFiles.some((file) => file.includes(name)),
			`Vite did not emit ${name}.wasm without the nipworker plugin`
		);
	}

	console.log(`Verified configuration-free Vite output: ${wasmFiles.join(', ')}`);
} finally {
	await rm(fixtureRoot, { recursive: true, force: true });
}
