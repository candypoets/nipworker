import * as flatbuffers from 'flatbuffers';
import { ArrayBufferReader } from 'src/lib/ArrayBufferReader';
import { BaseBackend, type StorageAdapter } from 'src/lib/BaseBackend';
import type { NostrManagerConfig, RequestObject, SubscriptionConfig } from 'src/types';
import type { EventTemplate, NostrEvent } from 'nostr-tools';
import {
	GetPublicKeyT,
	MainContent,
	MainMessageT,
	MessageType,
	MuteFilterPipeConfigT,
	ParsePipeConfigT,
	PipeConfig,
	PipelineConfigT,
	PipeT,
	Pubkey,
	PublishT,
	Raw,
	RequestT,
	SaveToDbPipeConfigT,
	SerializeEventsPipeConfigT,
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
import { setManager } from './manager';
import { scheduleMicrotask } from './lib/scheduleMicrotask';

declare const globalThis: {
	lynx?: {
		getNativeModules?: () => Record<string, any>;
		getStorageSync?: (opts: { key: string }) => { data: string } | undefined;
		setStorageSync?: (opts: { key: string; data: string }) => void;
		removeStorageSync?: (opts: { key: string }) => void;
		getJSModule?: (name: string) => any;
		getNativeApp?: () => any;
	};
	NativeModules?: Record<string, any>;
};

/** Lynx injects NativeModules as a bundle parameter, not on globalThis. */
declare const NativeModules: Record<string, any> | undefined;

/** In some Lynx runtimes the lynx global is bare, not on globalThis. */
declare const lynx:
	| {
			getJSModule?: (name: string) => any;
			getNativeModules?: () => Record<string, any>;
			getNativeApp?: () => any;
	  }
	| undefined;

type NativeBackendGlobalState = {
	instance: NativeBackend | undefined;
	activeListener:
		| {
				emitter: any;
				listener: (arg: any) => void;
				instanceId: string;
				eventName: string;
		  }
		| undefined;
	instances: Array<{ instanceId: string; stack?: string | undefined; t: number }>;
};

const NATIVE_BACKEND_STATE_KEY = '__nipworker_native_backend_state';
const LYNX_EVENT_NAME = 'NipworkerEvent';

export type NativeRuntimeBridge = {
	name: string;
	eventName: string;
	storage: StorageAdapter;
	getModule(): any;
	getEventEmitter(): any;
};

function getNativeBackendState(): NativeBackendGlobalState {
	const g = globalThis as any;
	if (!g[NATIVE_BACKEND_STATE_KEY]) {
		g[NATIVE_BACKEND_STATE_KEY] = {
			instance: undefined,
			activeListener: undefined,
			instances: []
		};
	}
	return g[NATIVE_BACKEND_STATE_KEY];
}

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
	}
};

function base64ToBytes(base64: string): Uint8Array {
	if (typeof atob === 'function') {
		const binary = atob(base64);
		const bytes = new Uint8Array(binary.length);
		for (let i = 0; i < binary.length; i++) {
			bytes[i] = binary.charCodeAt(i);
		}
		return bytes;
	}
	// Fallback decoder for environments without atob (e.g. some Lynx runtimes)
	const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
	const map = new Map<string, number>();
	for (let i = 0; i < alphabet.length; i++) {
		map.set(alphabet.charAt(i), i);
	}
	const clean = base64.replace(/=+$/, '');
	const len = clean.length;
	const bytes = new Uint8Array(Math.floor((len * 3) / 4));
	let j = 0;
	for (let i = 0; i < len; i += 4) {
		const ch0 = clean[i] ?? '';
		const ch1 = clean[i + 1] ?? '';
		const ch2 = clean[i + 2];
		const ch3 = clean[i + 3];
		const a = map.get(ch0) ?? 0;
		const b = map.get(ch1) ?? 0;
		const c = ch2 === undefined ? 0 : (map.get(ch2) ?? 0);
		const d = ch3 === undefined ? 0 : (map.get(ch3) ?? 0);
		bytes[j++] = (a << 2) | (b >> 4);
		if (ch2 !== undefined) bytes[j++] = ((b & 0x0f) << 4) | (c >> 2);
		if (ch3 !== undefined) bytes[j++] = ((c & 0x03) << 6) | d;
	}
	return bytes.subarray(0, j);
}

function eventDataToBytes(event: any): Uint8Array | null {
	if (event instanceof Uint8Array) {
		return event;
	}
	if (event instanceof ArrayBuffer) {
		return new Uint8Array(event);
	}
	if (ArrayBuffer.isView(event)) {
		return new Uint8Array(event.buffer, event.byteOffset, event.byteLength);
	}
	if (Array.isArray(event)) {
		return new Uint8Array(event);
	}
	if (typeof event === 'string') {
		return base64ToBytes(event);
	}
	if (!event || typeof event !== 'object') {
		return null;
	}
	if (event.v !== 1 || typeof event.data === 'undefined') {
		return null;
	}
	if (event.encoding === 'bytes') {
		return eventDataToBytes(event.data);
	}
	if (event.encoding === 'base64' && typeof event.data === 'string') {
		return base64ToBytes(event.data);
	}
	return null;
}

function getLynxNipworkerModule(): any {
	// In Sparkling/Lynx real builds, NativeModules is injected as a bundle
	// parameter (IIFE argument), not mounted on globalThis.
	let mod =
		(typeof NativeModules !== 'undefined' && NativeModules?.NipworkerLynxModule) ||
		globalThis.lynx?.getNativeModules?.()?.NipworkerLynxModule ||
		globalThis.NativeModules?.NipworkerLynxModule;

	if (!mod) {
		try {
			const app = globalThis.lynx?.getNativeApp?.();
			if (app && app.NativeModules) {
				mod = app.NativeModules.NipworkerLynxModule;
			}
		} catch {
			// ignore
		}
	}

	if (!mod) {
		throw new Error(
			'[NativeBackend] NipworkerLynxModule not found. Ensure the native module is registered.'
		);
	}
	return mod;
}

function getLynxEventEmitter(): any {
	let emitter = globalThis.lynx?.getJSModule?.('GlobalEventEmitter');
	if (!emitter && typeof lynx !== 'undefined') {
		emitter = lynx.getJSModule?.('GlobalEventEmitter');
	}
	if (!emitter) {
		throw new Error(
			'[NativeBackend] GlobalEventEmitter not found. Native events will not be received.'
		);
	}
	return emitter;
}

export const lynxNativeBridge: NativeRuntimeBridge = {
	name: 'lynx',
	eventName: LYNX_EVENT_NAME,
	storage: lynxStorageAdapter,
	getModule: getLynxNipworkerModule,
	getEventEmitter: getLynxEventEmitter
};

/**
 * NativeBackend implements the same public interface as EngineManager / NostrManager,
 * but communicates with the native libnipworker_native_ffi via a Lynx native module.
 *
 * This is a skeleton implementation for mobile (iOS / Android / HarmonyOS) consumption.
 */
export class NativeBackend extends BaseBackend {
	private nativeModule: any;
	private _signCB = (_event: NostrEvent) => {};
	private instanceId: string;
	private eventEmitter: any;
	private eventListener: ((arg: any) => void) | null = null;
	private deinitialized = false;
	private runtimeBridge: NativeRuntimeBridge;

	constructor(
		_config: NostrManagerConfig = {},
		runtimeBridge: NativeRuntimeBridge = lynxNativeBridge
	) {
		super(runtimeBridge.storage);
		this.runtimeBridge = runtimeBridge;
		this.instanceId = Math.random().toString(36).slice(2, 8);
		const nativeState = getNativeBackendState();
		nativeState.instances.push({
			instanceId: this.instanceId,
			stack: new Error().stack,
			t: Date.now()
		});
		this.nativeModule = runtimeBridge.getModule();
		if (typeof globalThis !== 'undefined') {
			(globalThis as any).__nipworker_native_diag = {
				callbackCount: 0,
				lastCallbackLen: 0,
				lastCallbackTime: 0,
				handleNativeMessageCount: 0,
				handleNativeMessageErrors: 0,
				subscriptionCount: 0,
				publishCount: 0
			};
		}
		// Probe the native environment and store diagnostics before any throw.
		const probe = {
			runtime: runtimeBridge.name,
			hasLynx: !!globalThis.lynx,
			hasGetJSModule: !!globalThis.lynx?.getJSModule,
			hasGlobalEventEmitter: false,
			hasNativeModulesGlobal: !!globalThis.NativeModules,
			hasNativeModulesFn: !!globalThis.lynx?.getNativeModules,
			hasNipworkerModule:
				!!globalThis.NativeModules?.NipworkerLynxModule ||
				!!globalThis.lynx?.getNativeModules?.()?.NipworkerLynxModule,
			emitterType: 'unknown',
			emitterHasAddListener: false
		};
		let emitter;
		try {
			emitter = runtimeBridge.getEventEmitter();
			probe.hasGlobalEventEmitter = !!emitter;
			probe.emitterType = typeof emitter;
			probe.emitterHasAddListener = typeof emitter?.addListener === 'function';
		} catch (e) {
			probe.emitterType = 'error:' + String(e);
		}
		if (typeof globalThis !== 'undefined') {
			const g = globalThis as any;
			g.__nipworker_native_probe = probe;
			(g.__nipworker_trace = g.__nipworker_trace || []).push({
				where: 'NativeBackend constructor probe',
				probe,
				t: Date.now()
			});
		}
		if (!emitter || typeof emitter.addListener !== 'function') {
			throw new Error(
				`[NativeBackend] ${runtimeBridge.name} event emitter unavailable: ${probe.emitterType}`
			);
		}
		// Register a persistent listener before starting the native engine.
		// Lynx may still send base64 envelopes; React Native sends byte arrays.
		this.eventEmitter = emitter;
		this.eventListener = (arg: any) => {
			if (this.deinitialized) return;
			const event = Array.isArray(arg) ? arg[0] : arg;
			let decoded: Uint8Array;
			try {
				const bytes = eventDataToBytes(event);
				if (!bytes) {
					console.warn('[NativeBackend] Ignoring malformed NipworkerEvent', event);
					return;
				}
				decoded = bytes;
			} catch (e) {
				console.error('[NativeBackend] Failed to decode native event payload', e);
				return;
			}
			if (typeof globalThis !== 'undefined') {
				const diag = (globalThis as any).__nipworker_native_diag;
				if (diag) {
					diag.callbackCount++;
					diag.lastCallbackLen = decoded.length;
					diag.lastCallbackTime = Date.now();
				}
			}
			try {
				this.handleNativeMessage(decoded);
			} catch (e) {
				console.error('[NativeBackend] Failed to handle native message', e);
				if (typeof globalThis !== 'undefined') {
					const diag = (globalThis as any).__nipworker_native_diag;
					if (diag) diag.handleNativeMessageErrors++;
				}
			}
		};
		if (nativeState.activeListener) {
			try {
				nativeState.activeListener.emitter.removeListener(
					nativeState.activeListener.eventName,
					nativeState.activeListener.listener
				);
				console.warn(
					'[NativeBackend] Removed stale NipworkerEvent listener from instanceId=' +
						nativeState.activeListener.instanceId
				);
			} catch (e) {
				console.warn('[NativeBackend] Failed to remove stale NipworkerEvent listener', e);
			}
		}
		emitter.addListener(runtimeBridge.eventName, this.eventListener);
		nativeState.activeListener = {
			emitter,
			listener: this.eventListener,
			instanceId: this.instanceId,
			eventName: runtimeBridge.eventName
		};
		nativeState.instance = this;
		this.nativeModule.init(_config);
		this.setupVisibilityTracking();
		scheduleMicrotask(() => this.restoreSession());
		// Auto-register so hooks work without explicit setManager() call
		setManager(this);
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
			}
		});
	}

	isDeinitialized(): boolean {
		return this.deinitialized;
	}

	private handleNativeMessage(data: Uint8Array): void {
		if (typeof globalThis !== 'undefined') {
			const diag = (globalThis as any).__nipworker_native_diag;
			if (diag) diag.handleNativeMessageCount++;
		}
		if (data.length < 8) {
			console.warn('[NativeBackend] Ignoring malformed native message: data too short');
			return;
		}
		const view = new DataView(data.buffer, data.byteOffset, data.byteLength);
		let offset = 0;
		const subIdLen = view.getUint32(offset, true);
		offset += 4;
		if (offset + subIdLen > data.byteLength) {
			console.warn('[NativeBackend] Ignoring malformed native message: subId exceeds buffer');
			return;
		}
		const subId = new TextDecoder().decode(data.subarray(offset, offset + subIdLen));
		offset += subIdLen;
		if (offset + 4 > data.byteLength) {
			console.warn('[NativeBackend] Ignoring malformed native message: missing payload length');
			return;
		}
		const payloadLen = view.getUint32(offset, true);
		offset += 4;
		if (offset + payloadLen > data.byteLength) {
			console.warn('[NativeBackend] Ignoring malformed native message: payload exceeds buffer');
			return;
		}
		const payload = data.subarray(offset, offset + payloadLen);

		if (subId === 'crypto') {
			this.handleCryptoMessage(payload);
			return;
		}
		if (subId === '') {
			this.handleDirectResponse(payload);
			return;
		}
		if (typeof globalThis !== 'undefined') {
			const diag = (globalThis as any).__nipworker_native_diag;
			if (diag) diag.subscriptionCount = this.subscriptions.size;
		}

		const subscription = this.subscriptions.get(subId);
		if (subscription) {
			const buf = subscription.buffer;
			const written = ArrayBufferReader.writePayload(buf, payload, subId);
			if (written) {
				this.dispatch(`subscription:${subId}`, subId);
			} else {
				this.closeSubscription(subId);
			}
			return;
		}

		const publish = this.publishes.get(subId);
		if (publish) {
			const written = ArrayBufferReader.writePayload(publish.buffer, payload, subId);
			if (written) {
				this.dispatch(`publish:${subId}`, subId);
			} else {
				this.publishes.delete(subId);
			}
			return;
		}

		console.warn(
			'[NativeBackend] Dropping native message for unknown subId=' +
				subId +
				', subscriptions=' +
				this.subscriptions.size +
				', publishes=' +
				this.publishes.size
		);
		if (typeof globalThis !== 'undefined') {
			const diag = (globalThis as any).__nipworker_native_diag;
			if (diag) diag.lastMissingSubId = subId;
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
						const sessionPayload =
							this._pendingSession.type === 'nip46' &&
							typeof msg.bunker_url === 'string' &&
							msg.bunker_url.startsWith('bunker://') &&
							this._pendingSession.payload &&
							typeof this._pendingSession.payload === 'object'
								? { ...this._pendingSession.payload, url: msg.bunker_url }
								: this._pendingSession.payload;
						this.saveSession(this.activePubkey!, this._pendingSession.type, sessionPayload);
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

	private closeSubscription(subId: string): void {
		const unsubscribeT = new UnsubscribeT(this.textEncoder.encode(subId));
		const mainT = new MainMessageT(MainContent.Unsubscribe, unsubscribeT);
		const builder = new flatbuffers.Builder(1024);
		builder.finish(mainT.pack(builder));
		this.postMessage(builder.asUint8Array());
		this.subscriptions.delete(subId);
	}

	subscribe(
		subscriptionId: string,
		requests: RequestObject[],
		options: SubscriptionConfig
	): ArrayBuffer {
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

		const pipeline =
			options.pipeline !== undefined
				? new PipelineConfigT(options.pipeline)
				: new PipelineConfigT([
						new PipeT(PipeConfig.MuteFilterPipeConfig, new MuteFilterPipeConfigT()),
						new PipeT(PipeConfig.ParsePipeConfig, new ParsePipeConfigT()),
						new PipeT(PipeConfig.SaveToDbPipeConfig, new SaveToDbPipeConfigT()),
						new PipeT(PipeConfig.SerializeEventsPipeConfig, new SerializeEventsPipeConfigT(subId))
					]);
		const optionsT = new SubscriptionConfigT(
			pipeline,
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
						r.search ? this.textEncoder.encode(r.search) : null,
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
		this.deinitialized = true;
		const nativeState = getNativeBackendState();
		if (this.eventEmitter && this.eventListener) {
			this.eventEmitter.removeListener(this.runtimeBridge.eventName, this.eventListener);
		}
		if (nativeState.activeListener?.listener === this.eventListener) {
			nativeState.activeListener = undefined;
		}
		if (nativeState.instance === this) {
			nativeState.instance = undefined;
		}
		this.eventEmitter = null;
		this.eventListener = null;
		this.nativeModule.deinit();
	}
}

export function getOrCreateNativeBackend(config: NostrManagerConfig = {}): NativeBackend {
	const nativeState = getNativeBackendState();
	if (nativeState.instance && !nativeState.instance.isDeinitialized()) {
		return nativeState.instance;
	}
	return new NativeBackend(config);
}
