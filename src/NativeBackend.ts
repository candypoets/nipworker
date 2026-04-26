import * as flatbuffers from 'flatbuffers';
import { ArrayBufferReader } from 'src/lib/ArrayBufferReader';
import { BaseBackend, type StorageAdapter } from 'src/lib/BaseBackend';
import { NostrManagerConfig, RequestObject, SubscriptionConfig } from 'src/types';
import type { EventTemplate, NostrEvent } from 'nostr-tools';
import {
	GetPublicKeyT,
	MainContent,
	MainMessageT,
	MessageType,
	PipelineConfigT,
	Pubkey,
	PublishT,
	Raw,
	RequestT,
	SignEventT,
	SignedEvent,
	StringVec,
	StringVecT,
	SubscribeT,
	SubscriptionConfigT,
	TemplateT,
	UnsubscribeT,
	WorkerMessage
} from './generated/nostr/fb';

declare const globalThis: {
	lynx?: {
		getNativeModules?: () => Record<string, any>;
		getStorageSync?: (opts: { key: string }) => { data: string } | undefined;
		setStorageSync?: (opts: { key: string; data: string }) => void;
		removeStorageSync?: (opts: { key: string }) => void;
	};
	NativeModules?: Record<string, any>;
};

/** Platform-aware storage: tries localStorage first, then Lynx storage. */
const lynxStorageAdapter: StorageAdapter = {
	getItem(key: string): string | null {
		if (typeof localStorage !== 'undefined') {
			return localStorage.getItem(key);
		}
		const result = globalThis.lynx?.getStorageSync?.({ key });
		return result?.data ?? null;
	},
	setItem(key: string, value: string): void {
		if (typeof localStorage !== 'undefined') {
			localStorage.setItem(key, value);
			return;
		}
		globalThis.lynx?.setStorageSync?.({ key, data: value });
	},
	removeItem(key: string): void {
		if (typeof localStorage !== 'undefined') {
			localStorage.removeItem(key);
			return;
		}
		globalThis.lynx?.removeStorageSync?.({ key });
	},
};

function getNipworkerModule(): any {
	const mod =
		globalThis.lynx?.getNativeModules?.()?.NipworkerLynxModule ||
		globalThis.NativeModules?.NipworkerLynxModule;
	if (!mod) {
		throw new Error(
			'[NativeBackend] NipworkerLynxModule not found. Ensure the native module is registered.'
		);
	}
	return mod;
}

/**
 * NativeBackend implements the same public interface as EngineManager / NostrManager,
 * but communicates with the native libnipworker_native_ffi via a Lynx native module.
 *
 * This is a skeleton implementation for mobile (iOS / Android / HarmonyOS) consumption.
 */
export class NativeBackend extends BaseBackend {
	private nativeModule: any;
	private _signCB = (_event: NostrEvent) => {};

	constructor(_config: NostrManagerConfig = {}) {
		super(lynxStorageAdapter);
		this.nativeModule = getNipworkerModule();
		this.nativeModule.init((data: ArrayBuffer) => {
			this.handleNativeMessage(new Uint8Array(data));
		});
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
				// TODO: native engine wake not yet implemented via FFI
				console.log('[NativeBackend] App returned to foreground');
			}
		});
	}

	private handleNativeMessage(data: Uint8Array): void {
		if (data.length < 8) return;
		const view = new DataView(data.buffer, data.byteOffset, data.byteLength);
		let offset = 0;
		const subIdLen = view.getUint32(offset, true);
		offset += 4;
		if (offset + subIdLen > data.byteLength) return;
		const subId = new TextDecoder().decode(data.subarray(offset, offset + subIdLen));
		offset += subIdLen;
		if (offset + 4 > data.byteLength) return;
		const payloadLen = view.getUint32(offset, true);
		offset += 4;
		if (offset + payloadLen > data.byteLength) return;
		const payload = data.subarray(offset, offset + payloadLen);

		if (subId === 'crypto') {
			this.handleCryptoMessage(payload);
			return;
		}
		if (subId === '') {
			this.handleDirectResponse(payload);
			return;
		}

		const subscription = this.subscriptions.get(subId);
		if (subscription) {
			const written = ArrayBufferReader.writeBatchedData(subscription.buffer, payload, subId);
			if (written) {
				this.dispatch(`subscription:${subId}`, subId);
			}
			return;
		}

		const publish = this.publishes.get(subId);
		if (publish) {
			const written = ArrayBufferReader.writeBatchedData(publish.buffer, payload, subId);
			if (written) {
				this.dispatch(`publish:${subId}`, subId);
			}
			return;
		}
	}

	private handleDirectResponse(payload: Uint8Array): void {
		if (payload.length < 4) return;
		const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
		const msgLen = view.getUint32(0, true);
		if (payload.length < 4 + msgLen) return;
		const bb = new flatbuffers.ByteBuffer(payload.subarray(4, 4 + msgLen));
		const workerMsg = WorkerMessage.getRootAsWorkerMessage(bb);
		const msgType = workerMsg.type();
		if (msgType === MessageType.Pubkey) {
			const pubkeyObj = workerMsg.content(new Pubkey());
			const pubkey = pubkeyObj ? pubkeyObj.pubkey() : null;
			if (pubkey) {
				this.activePubkey = pubkey;
				const secretKey =
					this._pendingSession?.type === 'privkey' ? this._pendingSession.payload : undefined;
				if (this._pendingSession) {
					this.saveSession(pubkey, this._pendingSession.type, this._pendingSession.payload);
					this._pendingSession = null;
				}
				this.dispatch('auth', { pubkey: this.activePubkey, hasSigner: true, secretKey });
			} else {
				this.dispatch('auth', { pubkey: null, hasSigner: false });
			}
		} else if (msgType === MessageType.SignedEvent) {
			const signedEventObj = workerMsg.content(new SignedEvent());
			const eventObj = signedEventObj ? signedEventObj.event() : null;
			if (eventObj) {
				const signedEvent: NostrEvent = {
					id: eventObj.id() || '',
					pubkey: eventObj.pubkey() || '',
					created_at: eventObj.createdAt(),
					kind: eventObj.kind(),
					tags: [],
					content: eventObj.content() || '',
					sig: eventObj.sig() || ''
				};
				for (let i = 0; i < eventObj.tagsLength(); i++) {
					const tag = eventObj.tags(i, new StringVec());
					if (tag) {
						const tagValues: string[] = [];
						for (let j = 0; j < tag.itemsLength(); j++) {
							const v = tag.items(j);
							if (v !== null) tagValues.push(v);
						}
						signedEvent.tags.push(tagValues);
					}
				}
				this._signCB(signedEvent);
			}
		}
	}

	private handleCryptoMessage(payload: Uint8Array): void {
		const bb = new flatbuffers.ByteBuffer(payload);
		const workerMsg = WorkerMessage.getRootAsWorkerMessage(bb);
		const msgType = workerMsg.type();
		if (msgType !== MessageType.Raw) return;
		const rawObj = workerMsg.content(new Raw());
		const rawStr = rawObj ? rawObj.raw() : null;
		if (!rawStr) return;
		try {
			const msg = JSON.parse(rawStr);
			if (msg.op === 'get_public_key' || msg.op === 'set_signer') {
				const secretKey =
					this._pendingSession?.type === 'privkey' ? this._pendingSession.payload : undefined;
				if (msg.result) {
					this.activePubkey = msg.result;
					if (this._pendingSession) {
						this.saveSession(this.activePubkey!, this._pendingSession.type, this._pendingSession.payload);
						this._pendingSession = null;
					}
				}
				this.dispatch('auth', { pubkey: this.activePubkey, hasSigner: !!msg.result, secretKey });
			} else if (msg.op === 'sign_event' && msg.result) {
				const parsed = JSON.parse(msg.result);
				this._signCB(parsed);
			} else if (msg.error) {
				console.warn('[NativeBackend] Crypto error:', msg.error);
			}
		} catch (e) {
			console.warn('[NativeBackend] Failed to parse crypto raw message:', e);
		}
	}

	private postMessage(bytes: Uint8Array): void {
		// Builder.asUint8Array() returns a view on a potentially larger backing buffer.
		// Copy to an exact-sized ArrayBuffer so the native bridge receives only valid bytes.
		this.nativeModule.handleMessage(bytes.slice().buffer);
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
		this.postMessage(uint8Array);

		return buffer;
	}

	publish(
		publish_id: string,
		event: NostrEvent,
		defaultRelays: string[] = [],
		optimisticSubIds?: string[]
	): ArrayBuffer {
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
		this.postMessage(uint8Array);
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
			case 'privkey': {
				this.nativeModule.setPrivateKey(payload as string);
				this.getPublicKey();
				break;
			}
			case 'nip07':
				console.warn('[NativeBackend] NIP-07 is not supported in the native backend');
				this.dispatch('auth', { pubkey: null, hasSigner: false });
				break;
			case 'nip46': {
				// TODO: native engine does not yet expose a proxy signer callback.
				console.warn('[NativeBackend] NIP-46 is not yet supported in the native backend');
				this.dispatch('auth', { pubkey: null, hasSigner: false });
				break;
			}
		}
	}

	signEvent(event: EventTemplate, cb: (event: NostrEvent) => void) {
		this._signCB = cb;
		const templateT = new TemplateT(
			event.kind,
			event.created_at,
			this.textEncoder.encode(event.content),
			event.tags.map((t) => new StringVecT(t)) || []
		);
		const signEventT = new SignEventT(templateT);
		const mainT = new MainMessageT(MainContent.SignEvent, signEventT);
		const builder = new flatbuffers.Builder(2048);
		builder.finish(mainT.pack(builder));
		const uint8Array = builder.asUint8Array();
		this.postMessage(uint8Array);
	}

	getPublicKey() {
		const mainT = new MainMessageT(MainContent.GetPublicKey, new GetPublicKeyT());
		const builder = new flatbuffers.Builder(2048);
		builder.finish(mainT.pack(builder));
		const uint8Array = builder.asUint8Array();
		this.postMessage(uint8Array);
	}

	protected onLogout(): void {
		// TODO: send clear_signer to native engine once supported via FFI
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
			this.postMessage(uint8Array);
			this.subscriptions.delete(subId);
		}
	}

	/**
	 * Explicitly tear down the native engine. Call this when the app is shutting
	 * down or the backend is no longer needed.
	 */
	deinit(): void {
		this.nativeModule.deinit();
	}
}
