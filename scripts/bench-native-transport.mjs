import { execFileSync } from 'node:child_process';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const sourceDir = join(root, 'crates/native-ffi/react-native/cpp');
const buildDir = mkdtempSync(join(tmpdir(), 'nipworker-native-transport-bench-'));
const executable = join(buildDir, 'native-transport-benchmark');
const compiler = process.env.CXX || 'c++';
const outputIndex = process.argv.indexOf('--output');
const outputPath = outputIndex >= 0 ? process.argv[outputIndex + 1] : undefined;
const eventCount = process.env.NIPWORKER_BENCH_EVENTS || '1000000';
const threadCount = process.env.NIPWORKER_BENCH_THREADS || '16';
const controlCount = process.env.NIPWORKER_BENCH_CONTROLS || '100000';

try {
	execFileSync(
		compiler,
		[
			'-std=c++17',
			'-O3',
			'-DNDEBUG',
			'-pthread',
			'-Wall',
			'-Wextra',
			'-Wpedantic',
			'-Werror',
			'-I',
			sourceDir,
			join(sourceDir, 'NipworkerReactNativeTransport.cpp'),
			join(sourceDir, 'tests/NipworkerReactNativeTransportBenchmark.cpp'),
			'-o',
			executable
		],
		{ cwd: root, stdio: 'inherit' }
	);
	const raw = execFileSync(executable, [eventCount, threadCount, controlCount], {
		cwd: root,
		encoding: 'utf8'
	}).trim();
	const report = JSON.parse(raw);
	const markdown = [
		'# React Native native-delivery benchmark',
		'',
		`- Route callbacks: ${report.eventCount.toLocaleString()} across ${report.threadCount} threads`,
		`- Dirty routes drained: ${report.routesDrained.toLocaleString()}`,
		`- New measured route scheduler wakes: ${report.routeScheduledWakes.toLocaleString()}`,
		`- Legacy modeled generated-emitter wakes: ${report.legacyModeledRouteWakes.toLocaleString()}`,
		`- Modeled route-wake reduction: ${report.modeledRouteWakeReductionPercent.toFixed(5)}%`,
		`- Legacy modeled route allocations/copies: ${report.legacyModeledJniByteArrayAllocations.toLocaleString()} JNI byte arrays and ${report.legacyModeledPayloadCopies.toLocaleString()} payload copies`,
		`- New subscription payload copies: ${report.newRoutePayloadCopies} (payload remains in the Rust-owned pinned buffer)`,
		`- Route enqueue throughput: ${Math.round(report.routeEventsPerSecond).toLocaleString()} events/s`,
		`- Route drain: ${report.routeDrainMs.toFixed(3)} ms`,
		`- Control-phase scheduler wakes: ${report.controlScheduledWakes.toLocaleString()}`,
		`- Control packets accepted/drained: ${report.acceptedControlPackets.toLocaleString()} / ${report.controlCount.toLocaleString()}`,
		`- Control packets dropped: ${report.droppedControlPackets.toLocaleString()} (${report.droppedControlBytes.toLocaleString()} bytes)`,
		`- Control enqueue throughput: ${Math.round(report.controlPacketsPerSecond).toLocaleString()} packets/s`,
		`- Control queue byte high-water: ${report.controlBytesHighWater.toLocaleString()} bytes`,
		`- Process maximum RSS: ${report.maximumRssKb.toLocaleString()} KiB`,
		'',
		'> Legacy counts are architecture-derived from the retained Android incident path: each native callback allocated/copied a JNI byte array, copied into the C++ vector queue, and emitted one generated event wake. They are not timings from rerunning the crashing binary. New figures are measured by this executable.',
		'',
		'```json',
		JSON.stringify(report, null, 2),
		'```',
		''
	].join('\n');
	if (outputPath) writeFileSync(resolve(outputPath), markdown);
	process.stdout.write(markdown);
} finally {
	rmSync(buildDir, { recursive: true, force: true });
}
