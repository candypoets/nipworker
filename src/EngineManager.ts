import * as flatbuffers from 'flatbuffers';
import { ArrayBufferReader } from 'src/lib/ArrayBufferReader';
import { NostrManagerConfig, RequestObject, SubscriptionConfig } from 'src/types';
import type { EventTemplate, NostrEvent } from 'nostr-tools';
import {
	GetPublicKeyT,
	MainContent,
	MainMessageT,
	PipelineConfigT,
	PublishT,
	RequestT,
	StringVecT,
	SubscribeT,
	SubscriptionConfigT,
	TemplateT,
	UnsubscribeT
} from './generated/nostr/fb';

/**
 * EngineManager is a single-worker backend for nipworker-core.
 * It spawns one WASM worker (nipworker-engine) that hosts the full
 * NostrEngine (transport + storage + parser + crypto) internally.
 */
export class EngineManager {
	private worker: Worker;
	private textEncoder = new TextEncoder();
	private subscriptions = new Map<
		string,
		{
			buffer: ArrayBuffer;
			options: SubscriptionConfig;
			refCount: number;
		}
	>();
	private publishes = new Map<string, { buffer: ArrayBuffer }>();
	private eventTarget = new EventTarget();
	private activePubkey: string | null = null;
	private _pendingSession: { type: string; payload: any } | null = null;
	private _signCB = (_event: NostrEvent) => {};

	private relayStatuses = new Map<
		string,
		{ status: 'connected' | 'failed' | 'close'; timestamp: number }
	>();

	public PERPETUAL_SUBSCRIPTIONS = ['notifications', 'starterpack'];

	constructor(_config: NostrManagerConfig = {}) {
		const engineURL = new URL('./engine/index.js', import.meta.url);
		this.worker = new Worker(engineURL, { type: 'module' });

		const mainPort = new MessageChannel();

		this.worker.postMessage(
			{ type: 'init', payload: { port: mainPort.port2 } },
			[mainPort.port2]
		);

		mainPort.port1.onmessage = (event) => {
			const { subId, data, type, status, url } = event.data;

			if (subId && data) {
				const subscription = this.subscriptions.get(subId);
				if (subscription) {
					const written = ArrayBufferReader.writeBatchedData(subscription.buffer, data, subId);
					if (written) {
						this.dispatch(`subscription:${subId}`, subId);
					}
					return;
				}
				const publish = this.publishes.get(subId);
				if (publish) {
					const written = ArrayBufferReader.writeBatchedData(publish.buffer, data, subId);
					if (written) {
						this.dispatch(`publish:${subId}`, subId);
					}
					return;
				}
			}

			if (type === 'relay:status' && url && status) {
				this.relayStatuses.set(url, { status, timestamp: Date.now() });
				this.dispatch('relay:status', { status, url });
			}

			if (type === 'response' && event.data.op === 'get_pubkey') {
				this.handleCryptoResponse(event.data);
			}
			if (type === 'response' && event.data.op === 'sign_event' && event.data.ok) {
				try {
					const parsed = JSON.parse(event.data.result);
					this._signCB(parsed);
				} catch (e) {
					console.warn('[EngineManager] Failed to parse signed event', e);
				}
			}
		};

		this.setupVisibilityTracking();
		queueMicrotask(() => this.restoreSession());
	}

	private setupVisibilityTracking(): void {
		if (typeof document === 'undefined') return;
		let wasHidden = false;
		document.addEventListener('visibilitychange', () => {
			if (document.hidden) {
				wasHidden = true;
			} else if (wasHidden) {
				wasHidden = false;
				this.worker.postMessage({ type: 'wake', source: 'visibility' });
			}
		});
	}

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

	private dispatch(type: string, detail?: unknown): void {
		this.eventTarget.dispatchEvent(new CustomEvent(type, { detail }));
	}

	private postMessage(message: any, transfer?: Transferable[]) {
		this.worker.postMessage(message, transfer || []);
	}

	private handleCryptoResponse(msg: any) {
		if (msg.op === 'get_pubkey') {
			if (msg.ok) {
				this.activePubkey = msg.result;
				if (this._pendingSession) {
					this.saveSession(this.activePubkey!, this._pendingSession.type, this._pendingSession.payload);
					this._pendingSession = null;
				}
			}
			const secretKey =
				this._pendingSession?.type === 'privkey' ? this._pendingSession.payload : undefined;
			this.dispatch('auth', { pubkey: this.activePubkey, hasSigner: msg.ok, secretKey });
		}
	}

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

	subscribe(subscriptionId: string, requests: RequestObject[], options: SubscriptionConfig): ArrayBuffer {
		const subId = this.createShortId(subscriptionId);
		const existing = this.subscriptions.get(subId);
		if (existing) {
			existing.refCount++;
			return existing.buffer;
		}

		const totalLimit = requests.reduce((sum, req) => sum + (req.limit || 100), 0);
		const bufferSize = ArrayBufferReader.calculateBufferSize(totalLimit, options.bytesPerEvent);
		const buffer = new ArrayBuffer(bufferSize);
		ArrayBufferReader.initializeBuffer(buffer);

		this.subscriptions.set(subId, { buffer, options, refCount: 1 });

		const optionsT = new SubscriptionConfigT(
			new PipelineConfigT(options.pipeline || []),
			options.closeOnEose,
			options.cacheFirst,
			options.timeoutMs ? BigInt(options.timeoutMs) : undefined,
			options.maxEvents,
			options.skipCache,
			options.force,
			options.bytesPerEvent,
			options.isSlow,
			options.pagination ? this.textEncoder.encode(options.pagination) : null
		);

		const subscribeT = new SubscribeT(
			this.textEncoder.encode(subId),
			requests.map(
				(r) =>
					new RequestT(
						r.ids,
						r.authors,
						r.kinds,
						Object.entries(r.tags || {}).flatMap(
							([key, values]) => new StringVecT([key, ...values])
						),
						r.limit,
						r.since,
						r.until,
						this.textEncoder.encode(r.search),
						r.relays,
						r.closeOnEOSE,
						r.cacheFirst,
						r.noCache
					)
			),
			optionsT
		);

		const mainT = new MainMessageT(MainContent.Subscribe, subscribeT);
		const builder = new flatbuffers.Builder(2048);
		builder.finish(mainT.pack(builder));
		const uint8Array = builder.asUint8Array();
		this.postMessage({ serializedMessage: uint8Array }, [uint8Array.buffer]);

		return buffer;
	}

	getBuffer(subId: string): ArrayBuffer | undefined {
		const existing = this.subscriptions.get(subId);
		if (existing) {
			existing.refCount++;
			return existing.buffer;
		}
		return undefined;
	}

	getRelayStatuses(): Map<string, { status: 'connected' | 'failed' | 'close'; timestamp: number }> {
		return new Map(this.relayStatuses);
	}

	unsubscribe(subscriptionId: string): void {
		const subId = subscriptionId.length < 64 ? subscriptionId : this.createShortId(subscriptionId);
		const subscription = this.subscriptions.get(subId);
		if (subscription) {
			subscription.refCount--;
		}
	}

	publish(publish_id: string, event: NostrEvent, defaultRelays: string[] = [], optimisticSubIds?: string[]): ArrayBuffer {
		const buffer = new ArrayBuffer(3072);
		ArrayBufferReader.initializeBuffer(buffer);

		const templateT = new TemplateT(
			event.kind,
			event.created_at,
			this.textEncoder.encode(event.content),
			event.tags.map((t) => new StringVecT(t)) || []
		);
		const publishT = new PublishT(
			this.textEncoder.encode(publish_id),
			templateT,
			defaultRelays,
			optimisticSubIds || []
		);
		const mainT = new MainMessageT(MainContent.Publish, publishT);
		const builder = new flatbuffers.Builder(2048);
		builder.finish(mainT.pack(builder));
		const uint8Array = builder.asUint8Array();
		this.postMessage({ serializedMessage: uint8Array }, [uint8Array.buffer]);
		this.publishes.set(publish_id, { buffer });
		return buffer;
	}

	setSigner(name: string, payload?: string | { url: string; clientSecret: string }): void {
		this._pendingSession = { type: name, payload };
		switch (name) {
			case 'pubkey':
				this.activePubkey = payload as string;
				this.saveSession(this.activePubkey, 'pubkey', payload);
				this._pendingSession = null;
				this.dispatch('auth', { pubkey: this.activePubkey, hasSigner: false });
				break;
			case 'privkey':
				this.postMessage({ type: 'set_private_key', payload });
				break;
			case 'nip07':
				// Not supported in engine mode yet
				console.warn('[EngineManager] NIP-07 not supported in engine mode');
				break;
			case 'nip46':
				// Not supported in engine mode yet
				console.warn('[EngineManager] NIP-46 not supported in engine mode');
				break;
		}
	}

	signEvent(event: EventTemplate, cb: (event: NostrEvent) => void) {
		this._signCB = cb;
		this.postMessage({ type: 'sign_event', payload: JSON.stringify(event) });
	}

	getPublicKey() {
		const mainT = new MainMessageT(MainContent.GetPublicKey, new GetPublicKeyT());
		const builder = new flatbuffers.Builder(2048);
		builder.finish(mainT.pack(builder));
		const uint8Array = builder.asUint8Array();
		this.postMessage({ serializedMessage: uint8Array }, [uint8Array.buffer]);
	}

	getActivePubkey(): string | null {
		return this.activePubkey;
	}

	getSubscriptionCount(): number {
		return this.subscriptions.size;
	}

	getAccounts(): Record<string, { type: string; payload: any }> {
		const accountsJson = localStorage.getItem('nostr_signer_accounts') || '{}';
		try {
			return JSON.parse(accountsJson);
		} catch (e) {
			return {};
		}
	}

	switchAccount(pubkey: string) {
		const accounts = this.getAccounts();
		const session = accounts[pubkey];
		if (session) {
			this.setSigner(session.type, session.payload);
		}
	}

	private saveSession(pubkey: string, type: string, payload: any) {
		const accounts = this.getAccounts();
		accounts[pubkey] = { type, payload };
		localStorage.setItem('nostr_signer_accounts', JSON.stringify(accounts));
		localStorage.setItem('nostr_active_pubkey', pubkey);
	}

	private restoreSession() {
		const activePubkey = localStorage.getItem('nostr_active_pubkey');
		if (activePubkey) {
			this.switchAccount(activePubkey);
		} else {
			this.dispatch('auth', { pubkey: null, hasSigner: false });
		}
	}

	public logout(): void {
		this._pendingSession = null;
		this.activePubkey = null;
		this.postMessage({ type: 'clear_signer' });
		localStorage.removeItem('nostr_active_pubkey');
		this.dispatch('logout');
	}

	public removeAccount(): void {
		const currentPubkey = this.activePubkey;
		if (!currentPubkey) return;
		const accounts = this.getAccounts();
		delete accounts[currentPubkey];
		localStorage.setItem('nostr_signer_accounts', JSON.stringify(accounts));
		const remaining = Object.keys(accounts);
		if (remaining.length > 0 && remaining[0]) {
			this.switchAccount(remaining[0]);
		} else {
			this.logout();
		}
	}

	cleanup(): void {
		const toDelete: string[] = [];
		for (const [subId, subscription] of this.subscriptions.entries()) {
			if (subscription.refCount <= 0 && !this.PERPETUAL_SUBSCRIPTIONS.includes(subId)) {
				toDelete.push(subId);
			}
		}
		for (const subId of toDelete) {
			const unsubscribeT = new UnsubscribeT(this.textEncoder.encode(subId));
			const mainT = new MainMessageT(MainContent.Unsubscribe, unsubscribeT);
			const builder = new flatbuffers.Builder(256);
			builder.finish(mainT.pack(builder));
			const uint8Array = builder.asUint8Array();
			this.postMessage({ serializedMessage: uint8Array }, [uint8Array.buffer]);
			this.subscriptions.delete(subId);
		}
	}
}
