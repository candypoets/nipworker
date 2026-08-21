import { execFileSync } from 'node:child_process';
import { readdirSync, readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import koffi from 'koffi';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const cycles = Number.parseInt(process.env.NIPWORKER_LIFECYCLE_CYCLES || '25', 10);
const settleMs = Number.parseInt(process.env.NIPWORKER_LIFECYCLE_SETTLE_MS || '40', 10);

if (process.platform !== 'linux') {
	throw new Error('The native lifecycle benchmark currently requires Linux /proc metrics.');
}
if (!Number.isSafeInteger(cycles) || cycles <= 0) {
	throw new Error('NIPWORKER_LIFECYCLE_CYCLES must be a positive integer.');
}
if (!Number.isSafeInteger(settleMs) || settleMs < 0) {
	throw new Error('NIPWORKER_LIFECYCLE_SETTLE_MS must be a non-negative integer.');
}

execFileSync('cargo', ['build', '--manifest-path', 'crates/native-ffi/Cargo.toml'], {
	cwd: root,
	stdio: 'inherit'
});

const libraryPath = join(root, 'crates/native-ffi/target/debug/libnipworker_native_ffi.so');
const library = koffi.load(libraryPath);
const callbackType = koffi.proto(
	'void NipworkerLifecycleCallback(void *userdata, const uint8_t *bytes, size_t size)'
);
const init = library.func(
	'void *nipworker_init(NipworkerLifecycleCallback *callback, void *userdata)'
);
const deinit = library.func('void nipworker_deinit(void *handle)');
const freeBytes = library.func('void nipworker_free_bytes(uint8_t *bytes, size_t size)');
const callback = koffi.register((_userdata, bytes, size) => {
	if (bytes && Number(size) > 0) freeBytes(bytes, size);
}, koffi.pointer(callbackType));

function threadCount() {
	return readdirSync('/proc/self/task').length;
}

function residentSetKiB() {
	const status = readFileSync('/proc/self/status', 'utf8');
	const match = /^VmRSS:\s+(\d+)\s+kB$/m.exec(status);
	if (!match) throw new Error('VmRSS is unavailable in /proc/self/status.');
	return Number.parseInt(match[1], 10);
}

function settle() {
	return new Promise((resolvePromise) => setTimeout(resolvePromise, settleMs));
}

const baselineThreads = threadCount();
const baselineRssKiB = residentSetKiB();

try {
	for (let cycle = 0; cycle < cycles; cycle++) {
		const handle = init(callback, null);
		if (!handle) throw new Error(`nipworker_init failed at cycle ${cycle + 1}`);
		await settle();
		deinit(handle);
		await settle();
	}
} finally {
	koffi.unregister(callback);
}

const finalThreads = threadCount();
const finalRssKiB = residentSetKiB();
const report = {
	cycles,
	settleMs,
	baselineThreads,
	finalThreads,
	threadGrowth: finalThreads - baselineThreads,
	baselineRssKiB,
	finalRssKiB,
	rssGrowthKiB: finalRssKiB - baselineRssKiB
};

process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
