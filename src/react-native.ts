/**
 * React Native entry point for @candypoets/nipworker.
 *
 * This module exports a NativeBackend wired to a React Native native module.
 * It contains no WASM imports and is intended to be consumed as:
 *
 *   import { createNostrManager } from '@candypoets/nipworker/react-native';
 */

import { NativeEventEmitter, NativeModules } from 'react-native';

import { NativeBackend, type NativeRuntimeBridge } from './NativeBackend';
import { getManager, setManager, setGlobalManager } from './manager';
import type { NostrManagerLike } from './manager';
import type { NostrManagerConfig } from './types';
import type { StorageAdapter } from './lib/BaseBackend';

const REACT_NATIVE_EVENT_NAME = 'NipworkerEvent';
const memoryStorage = new Map<string, string>();
let reactNativeBackendInstance: ReactNativeBackend | undefined;

const reactNativeStorageAdapter: StorageAdapter = {
	getItem(key: string): string | null {
		const mod = NativeModules.NipworkerReactNativeModule;
		if (typeof mod?.getStorageItem === 'function') {
			const value = mod.getStorageItem(key);
			return typeof value === 'string' ? value : null;
		}
		return memoryStorage.get(key) ?? null;
	},
	setItem(key: string, value: string): void {
		const mod = NativeModules.NipworkerReactNativeModule;
		if (typeof mod?.setStorageItem === 'function') {
			mod.setStorageItem(key, value);
			return;
		}
		memoryStorage.set(key, value);
	},
	removeItem(key: string): void {
		const mod = NativeModules.NipworkerReactNativeModule;
		if (typeof mod?.removeStorageItem === 'function') {
			mod.removeStorageItem(key);
			return;
		}
		memoryStorage.delete(key);
	}
};

function getReactNativeModule(): any {
	const mod = NativeModules.NipworkerReactNativeModule;
	if (!mod) {
		throw new Error(
			'[ReactNativeBackend] NipworkerReactNativeModule not found. Ensure the native module is linked.'
		);
	}
	return mod;
}

function bytesToBase64(input: Uint8Array | ArrayBuffer): string {
	const bytes = input instanceof Uint8Array ? input : new Uint8Array(input);
	const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
	let output = '';
	let i = 0;
	for (; i + 2 < bytes.length; i += 3) {
		const n = (bytes[i]! << 16) | (bytes[i + 1]! << 8) | bytes[i + 2]!;
		output += alphabet[(n >> 18) & 63];
		output += alphabet[(n >> 12) & 63];
		output += alphabet[(n >> 6) & 63];
		output += alphabet[n & 63];
	}
	if (i < bytes.length) {
		const a = bytes[i]!;
		const b = i + 1 < bytes.length ? bytes[i + 1]! : 0;
		const n = (a << 16) | (b << 8);
		output += alphabet[(n >> 18) & 63];
		output += alphabet[(n >> 12) & 63];
		output += i + 1 < bytes.length ? alphabet[(n >> 6) & 63] : '=';
		output += '=';
	}
	return output;
}

const reactNativeBridge: NativeRuntimeBridge = {
	name: 'react-native',
	eventName: REACT_NATIVE_EVENT_NAME,
	storage: reactNativeStorageAdapter,
	getModule(): any {
		const mod = getReactNativeModule();
		return {
			init(): void {
				mod.init();
			},
			handleMessage(bytes: Uint8Array): void {
				mod.handleMessage(bytesToBase64(bytes));
			},
			setPrivateKey(secret: string): void {
				mod.setPrivateKey(secret);
			},
			deinit(): void {
				mod.deinit();
			}
		};
	},
	getEventEmitter(): any {
		const emitter = new NativeEventEmitter(getReactNativeModule());
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
	constructor(config: NostrManagerConfig = {}) {
		super(config, reactNativeBridge);
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
	return !!NativeModules.NipworkerReactNativeModule;
}

export function hasNativeModule(): boolean {
	return hasReactNativeModule();
}

export { getManager, setManager, setGlobalManager };
export type { NostrManagerLike };
export type * from './types';
