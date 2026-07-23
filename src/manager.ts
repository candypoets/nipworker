import type { EventTemplate, NostrEvent } from 'nostr-tools';
import type { RequestObject, SubscriptionConfig } from './types';

/**
 * Common interface implemented by all backend variants:
 * - NostrManager   (4-worker WASM)
 * - ReactNativeManager (React Native native module)
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
	/** Pin a signed kind-0 profile as this device's visible nearby identity. */
	setMeshProfile(profile: NostrEvent): boolean;
	/** Stop sharing this device's profile while leaving mesh relay participation active. */
	clearMeshProfile(): boolean;
	cleanup(): void;
}

/**
 * Detect whether any native module backend is available.
 */
export function hasNativeModule(): boolean {
	return false;
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
