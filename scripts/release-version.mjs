import fs from 'node:fs';

const requestedVersion = process.argv[2];
const semver = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/;

function readJson(path) {
	return JSON.parse(fs.readFileSync(path, 'utf8'));
}

function writeJson(path, value) {
	fs.writeFileSync(path, `${JSON.stringify(value, null, 2)}\n`);
}

function nativeCargoVersion() {
	const cargo = fs.readFileSync('crates/native-ffi/Cargo.toml', 'utf8');
	const match = cargo.match(/^version\s*=\s*"([^"]+)"/m);
	if (!match) throw new Error('Could not read crates/native-ffi/Cargo.toml version');
	return match[1];
}

function assertReleaseVersions() {
	const packageJson = readJson('package.json');
	const packageLock = readJson('package-lock.json');
	const versions = new Map([
		['package.json', packageJson.version],
		['package-lock.json', packageLock.version],
		['package-lock.json root package', packageLock.packages?.['']?.version],
		['crates/native-ffi/Cargo.toml', nativeCargoVersion()]
	]);
	const mismatches = [...versions].filter(([, version]) => version !== packageJson.version);
	if (mismatches.length) {
		throw new Error(
			`Release versions do not match ${packageJson.version}:\n${mismatches
				.map(([path, version]) => `- ${path}: ${version ?? '<missing>'}`)
				.join('\n')}`
		);
	}
	console.log(`Release versions match: ${packageJson.version}`);
}

if (requestedVersion === '--check') {
	assertReleaseVersions();
	process.exit(0);
}

if (!requestedVersion || !semver.test(requestedVersion)) {
	throw new Error('Usage: node scripts/release-version.mjs <semver|--check>');
}

const packageJson = readJson('package.json');
packageJson.version = requestedVersion;
writeJson('package.json', packageJson);

const packageLock = readJson('package-lock.json');
packageLock.version = requestedVersion;
if (!packageLock.packages?.['']) {
	throw new Error('package-lock.json has no root package entry');
}
packageLock.packages[''].version = requestedVersion;
writeJson('package-lock.json', packageLock);

const cargoTomlPath = 'crates/native-ffi/Cargo.toml';
const cargoToml = fs.readFileSync(cargoTomlPath, 'utf8');
fs.writeFileSync(
	cargoTomlPath,
	cargoToml.replace(/^version\s*=\s*"[^"]+"/m, `version = "${requestedVersion}"`)
);

const cargoLockPath = 'crates/native-ffi/Cargo.lock';
const cargoLock = fs.readFileSync(cargoLockPath, 'utf8');
const nativePackage = /(\[\[package\]\]\nname = "nipworker-native-ffi"\nversion = ")[^"]+("\n)/;
if (!nativePackage.test(cargoLock)) {
	throw new Error('Could not find nipworker-native-ffi in Cargo.lock');
}
fs.writeFileSync(cargoLockPath, cargoLock.replace(nativePackage, `$1${requestedVersion}$2`));

assertReleaseVersions();
