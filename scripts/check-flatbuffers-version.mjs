import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';

const expectedVersion = readFileSync('.flatbuffers-version', 'utf8').trim();
const expectedGuard = expectedVersion.replaceAll('.', '_');
const failures = [];

function read(path) {
	return readFileSync(path, 'utf8');
}

function expectMatch(path, pattern, description) {
	if (!pattern.test(read(path))) {
		failures.push(`${path}: ${description}`);
	}
}

function expectLockVersion(path) {
	const match = read(path).match(/name = "flatbuffers"\nversion = "([^"]+)"/);
	if (match?.[1] !== expectedVersion) {
		failures.push(`${path}: resolved FlatBuffers ${match?.[1] ?? 'version not found'}`);
	}
}

if (process.argv.includes('--require-flatc')) {
	let flatcVersion;
	try {
		flatcVersion = execFileSync('flatc', ['--version'], { encoding: 'utf8' }).trim();
	} catch {
		failures.push(`flatc: executable not found; install ${expectedVersion}`);
	}

	if (flatcVersion && flatcVersion !== `flatc version ${expectedVersion}`) {
		failures.push(`flatc: expected ${expectedVersion}, found ${flatcVersion}`);
	}
}

const packageJson = JSON.parse(read('package.json'));
if (packageJson.peerDependencies?.flatbuffers !== expectedVersion) {
	failures.push(
		`package.json: expected exact peer dependency ${expectedVersion}, found ${packageJson.peerDependencies?.flatbuffers}`
	);
}

const packageLock = JSON.parse(read('package-lock.json'));
const npmVersion = packageLock.packages?.['node_modules/flatbuffers']?.version;
if (npmVersion !== expectedVersion) {
	failures.push(`package-lock.json: resolved FlatBuffers ${npmVersion ?? 'version not found'}`);
}

for (const path of [
	'crates/core/Cargo.toml',
	'crates/mesh/Cargo.toml',
	'crates/native-ffi/Cargo.toml'
]) {
	expectMatch(
		path,
		new RegExp(`^flatbuffers = "=${expectedVersion.replaceAll('.', '\\.')}"$`, 'm'),
		`expected exact FlatBuffers dependency =${expectedVersion}`
	);
}

for (const path of [
	'crates/cache/Cargo.lock',
	'crates/connections/Cargo.lock',
	'crates/core/Cargo.lock',
	'crates/crypto/Cargo.lock',
	'crates/mesh/Cargo.lock',
	'crates/native-ffi/Cargo.lock',
	'crates/parser/Cargo.lock'
]) {
	expectLockVersion(path);
}

for (const path of [
	'NipworkerReactNative.podspec',
	'crates/native-ffi/react-native/ios/NipworkerReactNative.podspec'
]) {
	expectMatch(
		path,
		new RegExp(`s\\.dependency 'FlatBuffers', '= ${expectedVersion.replaceAll('.', '\\.')}'`),
		`expected exact FlatBuffers dependency ${expectedVersion}`
	);
}

expectMatch(
	'swift/Package.swift',
	new RegExp(`exact: "${expectedVersion.replaceAll('.', '\\.')}"`),
	`expected exact FlatBuffers dependency ${expectedVersion}`
);
expectMatch(
	'crates/native-ffi/react-native/android/build.gradle',
	new RegExp(`flatbuffers-java:${expectedVersion.replaceAll('.', '\\.')}'`),
	`expected exact FlatBuffers dependency ${expectedVersion}`
);
expectMatch(
	'swift/Sources/NipworkerSwift/Generated/message_generated.swift',
	new RegExp(`FlatBuffersVersion_${expectedGuard}\\(\\)`),
	`expected generated version guard ${expectedVersion}`
);
expectMatch(
	'crates/native-ffi/react-native/android/src/main/java/nostr/fb/WorkerMessage.java',
	new RegExp(`FLATBUFFERS_${expectedGuard}\\(\\)`),
	`expected generated version guard ${expectedVersion}`
);

if (failures.length > 0) {
	console.error(
		`FlatBuffers ${expectedVersion} consistency check failed:\n- ${failures.join('\n- ')}`
	);
	process.exit(1);
}

const checkedCompiler = process.argv.includes('--require-flatc') ? 'compiler, ' : '';
console.log(
	`FlatBuffers ${checkedCompiler}runtimes, lockfiles, and generated guards use ${expectedVersion}`
);
