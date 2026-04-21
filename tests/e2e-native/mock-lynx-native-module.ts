/**
 * Mock NipworkerLynxModule for LynxJS testing environment.
 *
 * This module loads libnipworker_native_ffi.so via koffi and exposes
 * the same interface as the real Lynx native module (iOS/Android).
 *
 * Usage:
 *   globalThis.NativeModules = { NipworkerLynxModule: createMockNativeModule() };
 *
 * Architecture:
 *   JS (NativeBackend) → handleMessage(bytes) → koffi → Rust NostrEngine
 *   Rust NostrEngine → callback(bytes) → koffi → JS callback
 */

import koffi from 'koffi';
import * as path from 'path';
import * as fs from 'fs';

// C function signatures
const NIPWORKER_INIT_SIG = 'void *nipworker_init(void (*callback)(void *userdata, const uint8_t *ptr, size_t len), void *userdata)';
const NIPWORKER_HANDLE_MSG_SIG = 'void nipworker_handle_message(void *handle, const uint8_t *ptr, size_t len)';
const NIPWORKER_SET_PK_SIG = 'void nipworker_set_private_key(void *handle, const char *ptr)';
const NIPWORKER_DEINIT_SIG = 'void nipworker_deinit(void *handle)';
const NIPWORKER_FREE_BYTES_SIG = 'void nipworker_free_bytes(uint8_t *ptr, size_t len)';

interface NipworkerLynxModule {
	init(callback: (data: ArrayBuffer) => void): void;
	handleMessage(bytes: ArrayBuffer): void;
	setPrivateKey(secret: string): void;
	deinit(): void;
}

function findNativeLibrary(): string {
	const candidates = [
		// Development build (Linux)
		path.resolve(__dirname, '../../crates/native-ffi/target/release/libnipworker_native_ffi.so'),
		path.resolve(__dirname, '../../crates/native-ffi/target/release/libnipworker_native_ffi.dylib'),
		path.resolve(__dirname, '../../crates/native-ffi/target/release/libnipworker_native_ffi.dll'),
		// Installed package (relative to node_modules)
		path.resolve(__dirname, '../../../crates/native-ffi/target/release/libnipworker_native_ffi.so'),
	];

	for (const candidate of candidates) {
		if (fs.existsSync(candidate)) {
			return candidate;
		}
	}

	throw new Error(
		`libnipworker_native_ffi not found. ` +
		`Build it first: cd crates/native-ffi && cargo build --release\n` +
		`Searched: ${candidates.join(', ')}`
	);
}

/**
 * Create a mock NipworkerLynxModule backed by the real native FFI library.
 */
export function createMockNativeModule(): NipworkerLynxModule {
	const libPath = findNativeLibrary();
	const lib = koffi.load(libPath);

	const nipworkerInit = lib.func(NIPWORKER_INIT_SIG);
	const nipworkerHandleMessage = lib.func(NIPWORKER_HANDLE_MSG_SIG);
	const nipworkerSetPrivateKey = lib.func(NIPWORKER_SET_PK_SIG);
	const nipworkerDeinit = lib.func(NIPWORKER_DEINIT_SIG);
	const nipworkerFreeBytes = lib.func(NIPWORKER_FREE_BYTES_SIG);

	let handle: any = null;
	let jsCallback: ((data: ArrayBuffer) => void) | null = null;

	// Koffi callback that forwards Rust data back to JS.
	// The Rust side allocates the buffer and expects us to call nipworker_free_bytes.
	const nativeCallback = koffi.proto('void', 'nipworker_cb', ['void *', 'uint8_t *', 'size_t']);
	const callbackWrapper = koffi.register(
		(_userdata: any, ptr: bigint, len: number) => {
			if (!jsCallback || len === 0) return;
			// ptr is a pointer address as bigint; we need to read from it
			// koffi doesn't expose direct memory read, but we can use the free function
			// approach: create a temporary buffer view
			// Actually, koffi passes the pointer as a Buffer-like object on some platforms
			// Let's use a workaround: treat ptr as a pointer and read via koffi's decode
			try {
				const buf = koffi.decode(ptr, 'uint8_t', len);
				const arrayBuffer = new Uint8Array(buf).buffer;
				jsCallback(arrayBuffer);
				nipworkerFreeBytes(ptr, len);
			} catch (e) {
				console.error('[MockNativeModule] callback error:', e);
			}
		},
		nativeCallback
	);

	return {
		init(callback: (data: ArrayBuffer) => void) {
			jsCallback = callback;
			// Pass a dummy userdata (we don't need it since we close over jsCallback)
			handle = nipworkerInit(callbackWrapper, null);
			if (!handle) {
				throw new Error('[MockNativeModule] nipworker_init returned null');
			}
		},

		handleMessage(bytes: ArrayBuffer) {
			if (!handle) {
				throw new Error('[MockNativeModule] Engine not initialized. Call init() first.');
			}
			const uint8 = new Uint8Array(bytes);
			nipworkerHandleMessage(handle, uint8, uint8.length);
		},

		setPrivateKey(secret: string) {
			if (!handle) {
				throw new Error('[MockNativeModule] Engine not initialized. Call init() first.');
			}
			nipworkerSetPrivateKey(handle, secret);
		},

		deinit() {
			if (handle) {
				nipworkerDeinit(handle);
				handle = null;
			}
			jsCallback = null;
		},
	};
}
