import { execFileSync } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const sourceDir = join(root, 'crates/native-ffi/react-native/cpp');
const buildDir = mkdtempSync(join(tmpdir(), 'nipworker-native-transport-test-'));
const executable = join(buildDir, 'native-transport-test');
const compiler = process.env.CXX || 'c++';

try {
	execFileSync(
		compiler,
		[
			'-std=c++17',
			'-O2',
			'-pthread',
			'-Wall',
			'-Wextra',
			'-Wpedantic',
			'-Werror',
			'-DNIPWORKER_TRANSPORT_TESTING=1',
			'-I',
			sourceDir,
			join(sourceDir, 'NipworkerReactNativeTransport.cpp'),
			join(sourceDir, 'tests/NipworkerReactNativeTransportTest.cpp'),
			'-o',
			executable
		],
		{ cwd: root, stdio: 'inherit' }
	);
	execFileSync(executable, [], { cwd: root, stdio: 'inherit' });
} finally {
	rmSync(buildDir, { recursive: true, force: true });
}
