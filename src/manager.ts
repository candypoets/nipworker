import type { EventTemplate, NostrEvent } from 'nostr-tools';
import type { RequestObject, SubscriptionConfig } from './types';

/** Lynx injects NativeModules as a bundle parameter, not on globalThis. */
declare const NativeModules: Record<string, any> | undefined;

/**
 * Common interface implemented by all backend variants:
 * - NostrManager   (legacy 4-worker WASM)
 * - EngineManager  (single-worker WASM engine)
 * - NativeBackend  (LynxJS native module)
 */
export interface NostrManagerLike {
	readonly PERPETUAL_SUBSCRIPTIONS: string[];
	addEventListener(
		type: string,
		listener: EventListenerOrEventListenerObject,
		options?: AddEventListenerOptions
	): void;
	removeEventListener(
		type: string,
		listener: EventListenerOrEventListenerObject,
		options?: EventListenerOptions
	): void;
	createShortId(input: string): string;
	subscribe(
		subscriptionId: string,
		requests: RequestObject[],
		options: SubscriptionConfig
	): ArrayBuffer;
	getBuffer(subId: string): ArrayBuffer | undefined;
	getRelayStatuses(): Map<string, { status: 'connected' | 'failed' | 'close'; timestamp: number }>;
	unsubscribe(subscriptionId: string): void;
	publish(
		publish_id: string,
		event: any,
		defaultRelays?: string[],
		optimisticSubIds?: string[]
	): ArrayBuffer;
	releasePublish?(publish_id: string): void;
	setSigner(name: string, payload?: string | { url: string; clientSecret: string }): void;
	setNip46Bunker(bunkerUrl: string, clientSecret?: string): void;
	setNip46QR(nostrconnectUrl: string, clientSecret?: string): void;
	setNip07(): void;
	setPubkey(pubkey: string): void;
	signEvent(event: EventTemplate, cb: (event: NostrEvent) => void): void;
	getPublicKey(): void;
	getActivePubkey(): string | null;
	getSubscriptionCount(): number;
	getAccounts(): Record<string, { type: string; payload: any }>;
	switchAccount(pubkey: string): void;
	logout(): void;
	removeAccount(): void;
	cleanup(): void;
}

/**
 * Detect whether we are running inside a LynxJS environment with the
 * NipworkerLynxModule native module available.
 */
export function hasLynxNativeModule(): boolean {
	try {
		// In Sparkling/Lynx real builds, NativeModules is injected as a bundle
		// parameter (IIFE argument), not mounted on globalThis.
		let mod =
			(typeof NativeModules !== 'undefined' && (NativeModules as any)?.NipworkerLynxModule) ||
			(globalThis as any).lynx?.getNativeModules?.()?.NipworkerLynxModule ||
			(globalThis as any).NativeModules?.NipworkerLynxModule;

		if (!mod) {
			const app = (globalThis as any).lynx?.getNativeApp?.();
			if (app && app.NativeModules) {
				mod = app.NativeModules.NipworkerLynxModule;
			}
		}

		return !!mod;
	} catch {
		return false;
	}
}

/**
 * Detect whether any native module backend is available.
 */
export function hasNativeModule(): boolean {
	return hasLynxNativeModule();
}

// Global manager instance for hooks. Must be explicitly set by the app.
let globalManager: NostrManagerLike | null = null;

/**
 * Get the global manager instance used by hooks.
 */
export function getManager(): NostrManagerLike {
	if (!globalManager) {
		throw new Error(
			'[nipworker] Global manager is not set. Call setManager(createNostrManager(...)) before using hooks.'
		);
	}
	return globalManager;
}

/**
 * Set the global manager instance used by all hooks.
 * Call this early in your app before using any hooks.
 */
export function setManager(manager: NostrManagerLike): void {
	globalManager = manager;
}

/**
 * Backward-compatible alias for `setManager`.
 * @deprecated Use `setManager()`.
 */
export function setGlobalManager(manager: NostrManagerLike): void {
	setManager(manager);
}
