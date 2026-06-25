import type { EventTemplate, NostrEvent } from 'nostr-tools';
import type { RequestObject, SubscriptionConfig } from '../types';

// Minimal EventTarget / CustomEvent polyfill for environments without DOM APIs (QuickJS, Lynx, etc.)
class SimpleEvent {
	type: string;
	detail: any;
	bubbles: boolean;
	cancelable: boolean;
	defaultPrevented: boolean = false;
	constructor(type: string, options?: { detail?: any; bubbles?: boolean; cancelable?: boolean }) {
		this.type = type;
		this.detail = options?.detail;
		this.bubbles = options?.bubbles ?? false;
		this.cancelable = options?.cancelable ?? false;
	}
	preventDefault(): void {
		if (this.cancelable) this.defaultPrevented = true;
	}
}

class SimpleEventEmitter {
	private _listeners: Map<string, { listener: any; options: any }[]> = new Map();
	addEventListener(type: string, listener: any, options?: any): void {
		const arr = this._listeners.get(type) || [];
		arr.push({ listener, options });
		this._listeners.set(type, arr);
	}
	removeEventListener(type: string, listener: any, options?: any): void {
		const arr = this._listeners.get(type);
		if (!arr) return;
		this._listeners.set(type, arr.filter((l) => l.listener !== listener));
	}
	dispatchEvent(event: SimpleEvent): boolean {
		const arr = this._listeners.get(event.type);
		if (!arr) return true;
		arr.forEach(({ listener }) => {
			if (typeof listener === 'function') {
				listener(event);
			} else if (listener && typeof listener.handleEvent === 'function') {
				listener.handleEvent(event);
			}
		});
		return !event.defaultPrevented;
	}
}

/**
 * Storage adapter used by BaseBackend for session persistence.
 * Allows native backends (Lynx) to use platform storage instead of localStorage.
 */
export interface StorageAdapter {
	getItem(key: string): string | null;
	setItem(key: string, value: string): void;
	removeItem(key: string): void;
}

/** Browser localStorage adapter (default for web backends). */
export const localStorageAdapter: StorageAdapter = {
	getItem: (key) => localStorage.getItem(key),
	setItem: (key, value) => localStorage.setItem(key, value),
	removeItem: (key) => localStorage.removeItem(key),
};

/**
 * Abstract base class implementing shared logic across all NIPWorker backends:
 * - NostrManager   (legacy 4-worker WASM)
 * - EngineManager  (single-worker WASM engine)
 * - NativeBackend  (Lynx native module)
 *
 * Subclasses must implement the abstract communication methods
 * (subscribe, publish, setSigner, etc.) and may override session hooks.
 */
export abstract class BaseBackend {
	protected textEncoder = new TextEncoder();
	protected subscriptions = new Map<
		string,
		{
			buffer: ArrayBuffer;
			options: SubscriptionConfig;
			refCount: number;
		}
	>();
	protected publishes = new Map<string, { buffer: ArrayBuffer }>();
	protected eventTarget = new SimpleEventEmitter();
	protected activePubkey: string | null = null;
	protected _pendingSession: { type: string; payload: any } | null = null;
	protected relayStatuses = new Map<
		string,
		{ status: 'connected' | 'failed' | 'close'; timestamp: number }
	>();

	public PERPETUAL_SUBSCRIPTIONS = ['notifications', 'starterpack'];

	protected constructor(protected storage: StorageAdapter) {}

	// ── Event target plumbing ──────────────────────────────────────────

	public addEventListener(
		type: string,
		listener: EventListenerOrEventListenerObject,
		options?: AddEventListenerOptions
	): void {
		this.eventTarget.addEventListener(type, listener as EventListener, options);
	}

	public removeEventListener(
		type: string,
		listener: EventListenerOrEventListenerObject,
		options?: EventListenerOptions
	): void {
		this.eventTarget.removeEventListener(type, listener as EventListener, options);
	}

	protected dispatch(type: string, detail?: unknown): void {
		this.eventTarget.dispatchEvent(new SimpleEvent(type, { detail }));
	}

	// ── Shared helpers ─────────────────────────────────────────────────

	public createShortId(input: string): string {
		if (input.length < 64) return input;
		let hash = 0;
		for (let i = 0; i < input.length; i++) {
			const char = input.charCodeAt(i);
			hash = (hash << 5) - hash + char;
			hash = hash & hash;
		}
		const shortId = Math.abs(hash).toString(36);
		return shortId.substring(0, 63);
	}

	protected generateClientSecret(): string {
		if (typeof crypto === 'undefined' || !crypto.getRandomValues) {
			throw new Error('[BaseBackend] crypto.getRandomValues is not available in this environment');
		}
		const array = new Uint8Array(32);
		crypto.getRandomValues(array);
		return Array.from(array, (b) => b.toString(16).padStart(2, '0')).join('');
	}

	// ── Simple getters ─────────────────────────────────────────────────

	public getActivePubkey(): string | null {
		return this.activePubkey;
	}

	public getSubscriptionCount(): number {
		return this.subscriptions.size;
	}

	public getBuffer(subId: string): ArrayBuffer | undefined {
		const existing = this.subscriptions.get(subId);
		if (existing) {
			existing.refCount++;
			return existing.buffer;
		}
		return undefined;
	}

	public getRelayStatuses(): Map<string, { status: 'connected' | 'failed' | 'close'; timestamp: number }> {
		return new Map(this.relayStatuses);
	}

	public unsubscribe(subscriptionId: string): void {
		const subId = subscriptionId.length < 64 ? subscriptionId : this.createShortId(subscriptionId);
		const subscription = this.subscriptions.get(subId);
		if (subscription) {
			subscription.refCount--;
		}
	}

	// ── Session / account management ───────────────────────────────────

	public getAccounts(): Record<string, { type: string; payload: any }> {
		const accountsJson = this.storage.getItem('nostr_signer_accounts') || '{}';
		try {
			return JSON.parse(accountsJson);
		} catch (e) {
			return {};
		}
	}

	public switchAccount(pubkey: string) {
		const accounts = this.getAccounts();
		const session = accounts[pubkey];
		if (session) {
			this.setSigner(session.type, session.payload);
		}
	}

	protected saveSession(pubkey: string, type: string, payload: any) {
		const accounts = this.getAccounts();
		accounts[pubkey] = { type, payload };
		this.storage.setItem('nostr_signer_accounts', JSON.stringify(accounts));
		this.storage.setItem('nostr_active_pubkey', pubkey);
	}

	protected restoreSession() {
		const activePubkey = this.storage.getItem('nostr_active_pubkey');
		if (activePubkey) {
			this.switchAccount(activePubkey);
		} else {
			this.dispatch('auth', { pubkey: null, hasSigner: false });
		}
	}

	/** Hook called during logout so subclasses can notify their backend. */
	protected abstract onLogout(): void;

	public logout(): void {
		this._pendingSession = null;
		this.activePubkey = null;
		this.onLogout();
		this.storage.removeItem('nostr_active_pubkey');
		this.dispatch('logout');
	}

	public removeAccount(): void {
		const currentPubkey = this.activePubkey;
		if (!currentPubkey) return;

		const accounts = this.getAccounts();
		delete accounts[currentPubkey];
		this.storage.setItem('nostr_signer_accounts', JSON.stringify(accounts));

		const remaining = Object.keys(accounts);
		if (remaining.length > 0 && remaining[0]) {
			this.switchAccount(remaining[0]);
		} else {
			this.logout();
		}
	}

	// ── Signer convenience methods ─────────────────────────────────────

	public setNip46Bunker(bunkerUrl: string, clientSecret?: string): void {
		const secret = clientSecret || this.generateClientSecret();
		console.log('[BaseBackend] NIP-46 bunker:', bunkerUrl.slice(0, 50) + '...');
		this.setSigner('nip46', { url: bunkerUrl, clientSecret: secret });
	}

	public setNip46QR(nostrconnectUrl: string, clientSecret?: string): void {
		const secret = clientSecret || this.generateClientSecret();
		console.log('[BaseBackend] NIP-46 QR:', nostrconnectUrl.slice(0, 50) + '...');
		this.setSigner('nip46', { url: nostrconnectUrl, clientSecret: secret });
	}

	public setNip07(): void {
		this.setSigner('nip07', '');
	}

	public setPubkey(pubkey: string): void {
		this.setSigner('pubkey', pubkey);
	}

	// ── Abstract methods (backend-specific) ────────────────────────────

	public abstract subscribe(
		subscriptionId: string,
		requests: RequestObject[],
		options: SubscriptionConfig
	): ArrayBuffer;

	public abstract publish(
		publish_id: string,
		event: NostrEvent,
		defaultRelays?: string[],
		optimisticSubIds?: string[]
	): ArrayBuffer;

	public abstract setSigner(
		name: string,
		payload?: string | { url: string; clientSecret: string }
	): void;

	public abstract signEvent(event: EventTemplate, cb: (event: NostrEvent) => void): void;
	public abstract getPublicKey(): void;
	public abstract cleanup(): void;
}
