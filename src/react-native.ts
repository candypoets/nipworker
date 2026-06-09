/**
 * React Native entry point for @candypoets/nipworker.
 *
 * This module exports a NativeBackend wired to a React Native native module.
 * It contains no WASM imports and is intended to be consumed as:
 *
 *   import { createNostrManager } from '@candypoets/nipworker/react-native';
 */

import { AppState, NativeEventEmitter, NativeModules, type AppStateStatus } from 'react-native';

import { NativeBackend, type NativeRuntimeBridge } from './NativeBackend';
import { getManager, setManager, setGlobalManager } from './manager';
import type { NostrManagerLike } from './manager';
import type { NostrManagerConfig } from './types';
import type { StorageAdapter } from './lib/BaseBackend';
import NativeNipworkerReactNative from './specs/NativeNipworkerReactNative';

const REACT_NATIVE_EVENT_NAME = 'NipworkerEvent';
const memoryStorage = new Map<string, string>();
let reactNativeBackendInstance: ReactNativeBackend | undefined;

type ByteRuntime = {
	init(config?: NostrManagerConfig): void;
	handleMessage(bytes: ArrayBuffer): void;
	wake(): void;
	setPrivateKey(secret: string): void;
	deinit(): void;
	drain(): ArrayBuffer[];
};

function toExactUint8Array(bytes: Uint8Array | ArrayBuffer): Uint8Array {
	if (bytes instanceof Uint8Array) {
		return bytes.slice();
	}
	return new Uint8Array(bytes).slice();
}

function toExactArrayBuffer(bytes: Uint8Array): ArrayBuffer {
	const output = new ArrayBuffer(bytes.byteLength);
	new Uint8Array(output).set(bytes);
	return output;
}

function getByteRuntime(): ByteRuntime | undefined {
	return (globalThis as any).__nipworkerReactNativeByteRuntime;
}

function getTurboModule(): any {
	return NativeNipworkerReactNative;
}

function getAnyReactNativeModule(): any {
	return getTurboModule() ?? NativeModules.NipworkerReactNativeModule;
}

const reactNativeStorageAdapter: StorageAdapter = {
	getItem(key: string): string | null {
		const mod = getAnyReactNativeModule();
		if (typeof mod?.getStorageItem === 'function') {
			const value = mod.getStorageItem(key);
			return typeof value === 'string' ? value : null;
		}
		return memoryStorage.get(key) ?? null;
	},
	setItem(key: string, value: string): void {
		const mod = getAnyReactNativeModule();
		if (typeof mod?.setStorageItem === 'function') {
			mod.setStorageItem(key, value);
			return;
		}
		memoryStorage.set(key, value);
	},
	removeItem(key: string): void {
		const mod = getAnyReactNativeModule();
		if (typeof mod?.removeStorageItem === 'function') {
			mod.removeStorageItem(key);
			return;
		}
		memoryStorage.delete(key);
	}
};

function getReactNativeModule(): any {
	const mod = getAnyReactNativeModule();
	if (!mod) {
		throw new Error(
			'[ReactNativeBackend] NipworkerReactNative native module not found. Ensure the native module is linked.'
		);
	}
	return mod;
}

const reactNativeBridge: NativeRuntimeBridge = {
	name: 'react-native',
	eventName: REACT_NATIVE_EVENT_NAME,
	storage: reactNativeStorageAdapter,
	getModule(): any {
		const mod = getReactNativeModule();
		return {
			init(config?: NostrManagerConfig): void {
				const relayConfig = {
					defaultRelays: config?.defaultRelays ?? [],
					indexerRelays: config?.indexerRelays ?? []
				};
				const hasRelayConfig =
					relayConfig.defaultRelays.length > 0 || relayConfig.indexerRelays.length > 0;
				if (hasRelayConfig && typeof mod.initEngine === 'function') {
					mod.initEngine(relayConfig.defaultRelays, relayConfig.indexerRelays);
				}
				if (typeof mod.installByteRuntime === 'function') {
					mod.installByteRuntime();
				}
				const byteRuntime = getByteRuntime();
				if (byteRuntime) {
					byteRuntime.init(relayConfig);
					return;
				}
				if (typeof mod.initEngine === 'function') {
					mod.initEngine(relayConfig.defaultRelays, relayConfig.indexerRelays);
				} else {
					mod.init();
				}
			},
			handleMessage(bytes: Uint8Array | ArrayBuffer): void {
				const exact = toExactUint8Array(bytes);
				const byteRuntime = getByteRuntime();
				if (byteRuntime) {
					byteRuntime.handleMessage(toExactArrayBuffer(exact));
					return;
				}
				mod.handleMessage(Array.from(exact));
			},
			wake(): void {
				const byteRuntime = getByteRuntime();
				if (byteRuntime && typeof byteRuntime.wake === 'function') {
					byteRuntime.wake();
					return;
				}
				if (typeof mod.wake === 'function') {
					mod.wake();
				}
			},
			setPrivateKey(secret: string): void {
				const byteRuntime = getByteRuntime();
				if (byteRuntime) {
					byteRuntime.setPrivateKey(secret);
					return;
				}
				mod.setPrivateKey(secret);
			},
			deinit(): void {
				const byteRuntime = getByteRuntime();
				if (byteRuntime) {
					byteRuntime.deinit();
					return;
				}
				if (typeof mod.deinitEngine === 'function') {
					mod.deinitEngine();
				} else {
					mod.deinit();
				}
			}
		};
	},
	getEventEmitter(): any {
		const mod = getReactNativeModule();
		if (typeof mod.installByteRuntime === 'function') {
			mod.installByteRuntime();
		}
		const byteRuntime = getByteRuntime();
		if (byteRuntime) {
			const turbo = getTurboModule();
			const emitter = turbo?.onData ? undefined : new NativeEventEmitter(mod);
			const subscriptions = new Map<(event: any) => void, { remove: () => void }>();
			const deliverBuffers = (listener: (event: any) => void, buffers: ArrayBuffer[]) => {
				for (const buffer of buffers) {
					listener(buffer);
				}
			};
			const handleQueuedEvent = (listener: (event: any) => void, event: any) => {
				if (event?.v !== 1 || event?.encoding !== 'queued') return;
				deliverBuffers(listener, byteRuntime.drain());
			};
			return {
				addListener(_eventName: string, listener: (event: any) => void): void {
					const subscription = turbo?.onData
						? turbo.onData((event: any) => handleQueuedEvent(listener, event))
						: emitter!.addListener(REACT_NATIVE_EVENT_NAME, (event: any) =>
								handleQueuedEvent(listener, event)
							);
					subscriptions.set(listener, subscription);
				},
				removeListener(_eventName: string, listener: (event: any) => void): void {
					subscriptions.get(listener)?.remove();
					subscriptions.delete(listener);
				}
			};
		}

		const turbo = getTurboModule();
		if (turbo?.onData) {
			const subscriptions = new Map<(event: any) => void, { remove: () => void }>();
			return {
				addListener(_eventName: string, listener: (event: any) => void): void {
					subscriptions.set(
						listener,
						turbo.onData((event: { data: number[] }) => listener(event.data))
					);
				},
				removeListener(_eventName: string, listener: (event: any) => void): void {
					subscriptions.get(listener)?.remove();
					subscriptions.delete(listener);
				}
			};
		}

		const emitter = new NativeEventEmitter(mod);
		const subscriptions = new Map<(event: any) => void, { remove: () => void }>();
		return {
			addListener(eventName: string, listener: (event: any) => void): void {
				subscriptions.set(listener, emitter.addListener(eventName, listener));
			},
			removeListener(_eventName: string, listener: (event: any) => void): void {
				subscriptions.get(listener)?.remove();
				subscriptions.delete(listener);
			}
		};
	}
};

export class ReactNativeBackend extends NativeBackend {
	private appStateSubscription: { remove: () => void } | undefined;
	private appState = AppState.currentState;

	constructor(config: NostrManagerConfig = {}) {
		super(config, reactNativeBridge);
		this.appStateSubscription = AppState.addEventListener('change', (nextState: AppStateStatus) => {
			const wasBackgrounded = this.appState === 'background' || this.appState === 'inactive';
			this.appState = nextState;
			if (wasBackgrounded && nextState === 'active') {
				this.wakeNative();
			}
		});
	}

	override deinit(): void {
		this.appStateSubscription?.remove();
		this.appStateSubscription = undefined;
		super.deinit();
	}
}

export function getOrCreateReactNativeBackend(config: NostrManagerConfig = {}): ReactNativeBackend {
	if (reactNativeBackendInstance && !reactNativeBackendInstance.isDeinitialized()) {
		return reactNativeBackendInstance;
	}
	reactNativeBackendInstance = new ReactNativeBackend(config);
	return reactNativeBackendInstance;
}

export function createNostrManager(config?: NostrManagerConfig): NostrManagerLike {
	return getOrCreateReactNativeBackend(config);
}

export function hasReactNativeModule(): boolean {
	return !!getAnyReactNativeModule();
}

export function hasReactNativeTurboModule(): boolean {
	return !!getTurboModule();
}

export function hasReactNativeByteRuntime(): boolean {
	return !!getByteRuntime();
}

export function hasNativeModule(): boolean {
	return hasReactNativeModule();
}

export { getManager, setManager, setGlobalManager };
export type { NostrManagerLike };
export type * from './types';
