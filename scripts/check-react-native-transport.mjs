import { readFileSync } from 'node:fs';

const packageJson = JSON.parse(readFileSync('package.json', 'utf8'));

const paths = {
	jni: 'crates/native-ffi/android/nipworker_jni_impl.c',
	androidModule:
		'crates/native-ffi/react-native/android/src/main/java/com/candypoets/nipworker/reactnative/NipworkerReactNativeModule.kt',
	androidCmake: 'crates/native-ffi/react-native/android/CMakeLists.txt',
	androidCpp: 'crates/native-ffi/react-native/android/src/main/cpp/NipworkerByteRuntime.cpp',
	iosModule: 'crates/native-ffi/react-native/ios/NipworkerReactNativeModule.mm',
	swiftManager: 'swift/Sources/NipworkerSwift/NostrManager.swift',
	swiftFfi: 'swift/Sources/NipworkerSwift/FFI.swift',
	sharedJsiHeader: 'crates/native-ffi/react-native/cpp/NipworkerReactNativeJSI.h',
	sharedJsi: 'crates/native-ffi/react-native/cpp/NipworkerReactNativeJSI.cpp',
	reactNativeTs: 'src/react-native.ts',
	turboSpec: 'src/specs/NativeNipworkerReactNative.ts',
	rootPodspec: 'NipworkerReactNative.podspec',
	nestedPodspec: 'crates/native-ffi/react-native/ios/NipworkerReactNative.podspec'
};

const source = Object.fromEntries(
	Object.entries(paths).map(([name, path]) => [name, readFileSync(path, 'utf8')])
);
const failures = [];

for (const packagedSource of [
	'crates/native-ffi/react-native/cpp/NipworkerReactNativeTransport.h',
	'crates/native-ffi/react-native/cpp/NipworkerReactNativeTransport.cpp',
	'crates/native-ffi/react-native/cpp/NipworkerReactNativeJSI.h',
	'crates/native-ffi/react-native/cpp/NipworkerReactNativeJSI.cpp'
]) {
	if (!packageJson.files?.includes(packagedSource)) {
		failures.push(`package.json: files does not include ${packagedSource}`);
	}
}

function requireMatch(name, pattern, explanation) {
	if (!pattern.test(source[name])) failures.push(`${paths[name]}: ${explanation}`);
}

function rejectMatch(name, pattern, explanation) {
	if (pattern.test(source[name])) failures.push(`${paths[name]}: ${explanation}`);
}

for (const name of ['androidCmake', 'rootPodspec', 'nestedPodspec']) {
	requireMatch(
		name,
		/NipworkerReactNativeTransport/,
		'does not reference the shared React Native transport core'
	);
}

for (const name of ['androidCmake', 'rootPodspec', 'nestedPodspec']) {
	requireMatch(name, /NipworkerReactNativeJSI/, 'does not compile the shared JSI runtime adapter');
}

rejectMatch(
	'jni',
	/CallStaticVoidMethod|\bonNativeData\b|\bg_mid\b/,
	'retains the per-packet JNI/Kotlin callback path'
);
rejectMatch('jni', /AttachCurrentThread/, 'attaches arbitrary Rust callback threads to the JVM');
rejectMatch(
	'androidModule',
	/\bNipworkerRuntimeListener\b|\bemitRuntimeData\b|\bnativeQueueData\b|\bemitOnData\s*\(|\bonNativeData\b/,
	'retains Kotlin listener/event-emitter delivery on the hot path'
);
rejectMatch(
	'androidCpp',
	/\bgQueuedPackets\b|\bdrainBytes\b|\bgInstalled\b|\bnativeQueueData\b|\bnativeIsByteRuntimeInstalled\b/,
	'retains the old process-global byte queue or installation flag'
);
rejectMatch(
	'iosModule',
	/\bNipworkerQueuedPackets\b|\bNipworkerDrainPackets\b|\bNipworkerByteRuntimeAddress\b|\bNipworkerByteRuntimeInstalled\b|\bNipworkerRuntimeDataNotification\b/,
	'retains process-global queue or runtime installation state'
);
rejectMatch(
	'iosModule',
	/dispatch_get_main_queue|\bemitOnData\s*\(|\bsupportedEvents\b|\b_eventEmitterCallback\b/,
	'retains main-queue/generated-emitter delivery on the hot path'
);
rejectMatch(
	'swiftManager',
	/\bNipworkerRuntimeDataNotification\b|\bsharedRuntimeObserver\b|\breactNativeShared\s*\(|\bborrowedHandle\b/,
	'retains the Swift notification/shared-handle React Native delivery fallback'
);
rejectMatch(
	'swiftFfi',
	/\bnipworker_react_native_shared_handle_if_available\b|\bsharedHandle\b|\bnipworker_react_native_shared_handle\b/,
	'retains dynamic process-wide React Native shared-handle lookup'
);
for (const name of ['sharedJsiHeader', 'sharedJsi']) {
	rejectMatch(
		name,
		/\bsetObserver\b|\bEngineHost\s*::\s*Observer\b|\bEngineHostObserver\b/,
		'retains an EngineHost observer callback delivery fallback'
	);
}
rejectMatch(
	'reactNativeTs',
	/\bNativeEventEmitter\b|\bREACT_NATIVE_EVENT_NAME\b|\beventDataToBytes\b|\bencoding\s*===?\s*['"]queued['"]|\bturbo\?\.onData\b/,
	'retains the generated/legacy event-emitter compatibility path'
);
rejectMatch(
	'turboSpec',
	/CodegenTypes\.EventEmitter|\breadonly\s+onData\b/,
	'retains the generated onData emitter contract'
);
requireMatch(
	'androidCpp',
	/NipworkerReactNativeJSI|RuntimeTransport/,
	'does not install the shared runtime transport adapter'
);
requireMatch(
	'iosModule',
	/NipworkerReactNativeJSI|RuntimeTransport/,
	'does not install the shared runtime transport adapter'
);
requireMatch(
	'sharedJsi',
	/invokeAsync/,
	'does not schedule wakes through the React Native CallInvoker'
);
requireMatch(
	'androidModule',
	/override fun invalidate\(\)[\s\S]*invalidateTransport\(\)[\s\S]*NipworkerRuntime\.deinit\(\)/,
	'does not invalidate the runtime transport before deinitializing its engine'
);
requireMatch(
	'iosModule',
	/- \(void\)invalidate[\s\S]*\[self invalidateTransport\][\s\S]*EngineHost::shared\(\)\.deinit\(\)/,
	'does not invalidate the runtime transport before deinitializing its engine'
);

if (failures.length > 0) {
	console.error(`React Native transport parity check failed:\n- ${failures.join('\n- ')}`);
	process.exit(1);
}

console.log('React Native transport parity check passed');
