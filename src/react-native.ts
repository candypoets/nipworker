/**
 * React Native entry point for @candypoets/nipworker.
 *
 * This module exports a ReactNativeManager wired to a React Native native module.
 * It contains no WASM imports and is intended to be consumed as:
 *
 *   import { createNostrManager } from '@candypoets/nipworker/react-native';
 */

import { AppState, NativeEventEmitter, NativeModules, type AppStateStatus } from 'react-native';
import * as flatbuffers from 'flatbuffers';

import { BaseBackend, type StorageAdapter } from './lib/BaseBackend';
import { getManager, setManager, setGlobalManager } from './manager';
import type { NostrManagerLike } from './manager';
import type { NostrManagerConfig, RequestObject, SubscriptionConfig } from './types';
import type { EventTemplate, NostrEvent } from 'nostr-tools';
import {
	GetPublicKeyT,
	ConnectionStatus,
	MainContent,
	MainMessageT,
	Message,
	MuteFilterPipeConfigT,
	Nip46BunkerT,
	Nip46QRT,
	NostrEvent as FbNostrEvent,
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
	SetSignerResponse,
	SetSignerT,
	SignEventT,
	SignedEvent,
	SignerType,
	StringVec,
	StringVecT,
	SubscribeT,
	SubscriptionConfigT,
	TemplateT,
	WorkerMessage
} from './generated/nostr/fb';
import NativeNipworkerReactNative from './specs/NativeNipworkerReactNative';

const REACT_NATIVE_EVENT_NAME = 'NipworkerEvent';
const memoryStorage = new Map<string, string>();
let reactNativeBackendInstance: ReactNativeManager | undefined;

type ByteRuntime = {
	init(config?: NostrManagerConfig): void;
	handleMessage(bytes: ArrayBuffer): void;
	wake(): void;
	setPrivateKey(secret: string): void;
	deinit(): void;
	drain(): ArrayBuffer[];
	subscribe?(bytes: ArrayBuffer, subId: string): ArrayBuffer | undefined;
	publish?(bytes: ArrayBuffer, publishId: string): ArrayBuffer | undefined;
	registerSubscription?(subId: string, bufferSize: number): boolean;
	registerPublishBuffer?(publishId: string, bufferSize: number): boolean;
	retainSubscriptionBuffer?(subId: string): ArrayBuffer | undefined;
	retainSubscription?(subId: string): boolean;
	releaseSubscription?(subId: string): void;
	getSubscriptionBuffer?(subId: string): ArrayBuffer | undefined;
	cleanupSubscriptions?(): void;
};

type ReactNativeModuleFacade = {
	init(config?: NostrManagerConfig): void;
	handleMessage(bytes: Uint8Array | ArrayBuffer): void;
	wake(): void;
	setPrivateKey(secret: string): void;
	setMeshProfile(profileJson: string): boolean;
	clearMeshProfile(): boolean;
	deinit(): void;
	subscribe(bytes: Uint8Array | ArrayBuffer, subId: string): ArrayBuffer | undefined;
	publish(bytes: Uint8Array | ArrayBuffer, publishId: string): ArrayBuffer | undefined;
	registerSubscription(subId: string, bufferSize: number): boolean;
	registerPublishBuffer(publishId: string, bufferSize: number): boolean;
	retainSubscriptionBuffer(subId: string): ArrayBuffer | undefined;
	retainSubscription(subId: string): boolean;
	releaseSubscription(subId: string): void;
	getSubscriptionBuffer(subId: string): ArrayBuffer | undefined;
	cleanupSubscriptions(): void;
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

const reactNativeBridge = {
	name: 'react-native',
	eventName: REACT_NATIVE_EVENT_NAME,
	storage: reactNativeStorageAdapter,
	getModule(): ReactNativeModuleFacade {
		const mod = getReactNativeModule();
		return {
			init(config?: NostrManagerConfig): void {
				const relayConfig = {
					defaultRelays: config?.defaultRelays ?? [],
					indexerRelays: config?.indexerRelays ?? [],
					meshBLEEnabled: config?.meshBLEEnabled ?? false
				};
				// The shared native handle must be configured before installing the
				// byte runtime, which borrows that same handle.
				if (typeof mod.initEngine === 'function') {
					mod.initEngine(
						relayConfig.defaultRelays,
						relayConfig.indexerRelays,
						relayConfig.meshBLEEnabled
					);
				}
				if (typeof mod.installByteRuntime === 'function') {
					mod.installByteRuntime();
				}
				if (relayConfig.meshBLEEnabled && typeof mod.startMesh === 'function') {
					mod.startMesh();
				}
				const byteRuntime = getByteRuntime();
				if (byteRuntime) {
					byteRuntime.init(relayConfig);
					return;
				}
				if (typeof mod.initEngine !== 'function') {
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
			subscribe(bytes: Uint8Array | ArrayBuffer, subId: string): ArrayBuffer | undefined {
				const exact = toExactUint8Array(bytes);
				const byteRuntime = getByteRuntime();
				return byteRuntime?.subscribe?.(toExactArrayBuffer(exact), subId);
			},
			publish(bytes: Uint8Array | ArrayBuffer, publishId: string): ArrayBuffer | undefined {
				const exact = toExactUint8Array(bytes);
				const byteRuntime = getByteRuntime();
				return byteRuntime?.publish?.(toExactArrayBuffer(exact), publishId);
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
			setMeshProfile(profileJson: string): boolean {
				return typeof mod.setMeshProfile === 'function' && Boolean(mod.setMeshProfile(profileJson));
			},
			clearMeshProfile(): boolean {
				return typeof mod.clearMeshProfile === 'function' && Boolean(mod.clearMeshProfile());
			},
			deinit(): void {
				if (typeof mod.stopMesh === 'function') {
					mod.stopMesh();
				}
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
			},
			registerSubscription(subId: string, bufferSize: number): boolean {
				const byteRuntime = getByteRuntime();
				return byteRuntime?.registerSubscription?.(subId, bufferSize) === true;
			},
			registerPublishBuffer(publishId: string, bufferSize: number): boolean {
				const byteRuntime = getByteRuntime();
				return byteRuntime?.registerPublishBuffer?.(publishId, bufferSize) === true;
			},
			retainSubscriptionBuffer(subId: string): ArrayBuffer | undefined {
				const byteRuntime = getByteRuntime();
				return byteRuntime?.retainSubscriptionBuffer?.(subId);
			},
			retainSubscription(subId: string): boolean {
				const byteRuntime = getByteRuntime();
				return byteRuntime?.retainSubscription?.(subId) === true;
			},
			releaseSubscription(subId: string): void {
				const byteRuntime = getByteRuntime();
				byteRuntime?.releaseSubscription?.(subId);
			},
			getSubscriptionBuffer(subId: string): ArrayBuffer | undefined {
				const byteRuntime = getByteRuntime();
				return byteRuntime?.getSubscriptionBuffer?.(subId);
			},
			cleanupSubscriptions(): void {
				const byteRuntime = getByteRuntime();
				byteRuntime?.cleanupSubscriptions?.();
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
						turbo.onData((event: { data?: number[] }) => listener(event))
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

function eventDataToBytes(event: any): Uint8Array | null {
	if (event instanceof Uint8Array) return event;
	if (event instanceof ArrayBuffer) return new Uint8Array(event);
	if (ArrayBuffer.isView(event))
		return new Uint8Array(event.buffer, event.byteOffset, event.byteLength);
	if (Array.isArray(event)) return new Uint8Array(event);
	if (!event || typeof event !== 'object') return null;
	if (event.v === 1 && event.encoding === 'bytes') return eventDataToBytes(event.data);
	return null;
}

const ROUTE_WAKE_MAGIC = [0x4e, 0x57, 0x52, 0x31]; // NWR1

function decodeRouteWakeFrame(data: Uint8Array): string | null {
	if (data.byteLength < 8) return null;
	for (let i = 0; i < ROUTE_WAKE_MAGIC.length; i++) {
		if (data[i] !== ROUTE_WAKE_MAGIC[i]) return null;
	}
	const view = new DataView(data.buffer, data.byteOffset, data.byteLength);
	const subIdLength = view.getUint32(4, true);
	if (subIdLength === 0 || subIdLength !== data.byteLength - 8) return null;
	try {
		return new TextDecoder().decode(data.subarray(8));
	} catch {
		return null;
	}
}

export class ReactNativeManager extends BaseBackend {
	private appStateSubscription: { remove: () => void } | undefined;
	private appState = AppState.currentState;
	private nativeModule: ReactNativeModuleFacade;
	private eventEmitter: any;
	private eventListener: ((arg: any) => void) | undefined;
	private deinitialized = false;
	private _signRequests = new Map<number, (event: NostrEvent) => void>();
	private _nextSignRequestId = 1;
	private readonly useByteRuntime: boolean;

	constructor(config: NostrManagerConfig = {}) {
		super(reactNativeStorageAdapter);
		this.nativeModule = reactNativeBridge.getModule();
		this.nativeModule.init(config);
		this.useByteRuntime = !!getByteRuntime();
		this.eventEmitter = reactNativeBridge.getEventEmitter();
		this.eventListener = (arg: any) => {
			if (this.deinitialized) return;
			const event = Array.isArray(arg) ? arg[0] : arg;
			if (this.useByteRuntime && event?.v === 1 && event?.encoding === 'queued') {
				const byteRuntime = getByteRuntime();
				for (const payload of byteRuntime?.drain?.() ?? []) {
					this.handleNativeWake(new Uint8Array(payload));
				}
				return;
			}
			const decoded = eventDataToBytes(event);
			if (!decoded) return;
			if (this.useByteRuntime) {
				this.handleNativeWake(decoded);
				return;
			}
			this.handleNativeMessage(decoded);
		};
		this.eventEmitter.addListener(REACT_NATIVE_EVENT_NAME, this.eventListener);
		this.appStateSubscription = AppState.addEventListener('change', (nextState: AppStateStatus) => {
			const wasActive = this.appState === 'active';
			const wasBackgrounded = this.appState === 'background' || this.appState === 'inactive';
			const isBackgrounded = nextState === 'background' || nextState === 'inactive';
			this.appState = nextState;
			if (wasActive && isBackgrounded) {
				this.cleanup();
			}
			if (wasBackgrounded && nextState === 'active') {
				this.nativeModule.wake();
			}
		});
		setManager(this);
		Promise.resolve().then(() => this.restoreSession());
	}

	isDeinitialized(): boolean {
		return this.deinitialized;
	}

	private postMessage(bytes: Uint8Array): void {
		this.nativeModule.handleMessage(bytes.slice().buffer);
	}

	private handleNativeWake(data: Uint8Array): void {
		const routeSubId = decodeRouteWakeFrame(data);
		if (routeSubId) {
			this.dispatch(`subscription:${routeSubId}`, routeSubId);
			this.dispatch(`publish:${routeSubId}`, routeSubId);
			return;
		}
		this.handleNativePayload(data);
	}

	private handleNativeMessage(data: Uint8Array): void {
		const routeSubId = decodeRouteWakeFrame(data);
		if (routeSubId) {
			this.dispatch(`subscription:${routeSubId}`, routeSubId);
			this.dispatch(`publish:${routeSubId}`, routeSubId);
			return;
		}
		this.handleNativePayload(data);
	}

	private handleNativePayload(data: Uint8Array): void {
		let subId = '';
		let workerMsg: WorkerMessage;
		try {
			const bb = new flatbuffers.ByteBuffer(data);
			workerMsg = WorkerMessage.getRootAsWorkerMessage(bb);
			subId = workerMsg.subId() ?? '';
		} catch {
			return;
		}
		if (this.handleRelayStatus(workerMsg, subId)) {
			return;
		}
		if (subId === 'crypto' || subId === '') {
			console.log('[nipworker-rn] signer response routed', {
				subId,
				contentType: workerMsg.contentType()
			});
		}
		if (subId === 'crypto') {
			this.handleCryptoMessage(data);
			return;
		}
		if (subId === '') {
			const contentType = workerMsg.contentType();
			if (contentType === Message.SetSignerResponse || contentType === Message.Raw) {
				this.handleCryptoMessage(data);
				return;
			}
			this.handleDirectResponse(data);
			return;
		}
		this.dispatch(`subscription:${subId}`, subId);
		this.dispatch(`publish:${subId}`, subId);
	}

	private handleRelayStatus(workerMsg: WorkerMessage, subId: string): boolean {
		if (workerMsg.contentType() !== Message.ConnectionStatus) {
			return false;
		}
		const statusObj = workerMsg.content(new ConnectionStatus());
		const url = statusObj?.relayUrl() ?? '';
		const status = statusObj?.status() ?? '';
		if (url && status) {
			this.relayStatuses.set(url, {
				status: status as 'connected' | 'failed' | 'close',
				timestamp: Date.now()
			});
			this.dispatch('relay:status', { status, url });
		}
		return !subId;
	}

	private handleDirectResponse(payload: Uint8Array): void {
		if (payload.length < 4) return;
		const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
		const msgLen = view.getUint32(0, true);
		const maybeLenPrefixed = payload.length >= 4 + msgLen && msgLen > 0;
		const bb = new flatbuffers.ByteBuffer(
			maybeLenPrefixed ? payload.subarray(4, 4 + msgLen) : payload
		);
		const workerMsg = WorkerMessage.getRootAsWorkerMessage(bb);
		if (workerMsg.contentType() === Message.SignedEvent) {
			const signedEventObj = workerMsg.content(new SignedEvent());
			const eventObj = signedEventObj ? signedEventObj.event() : null;
			if (!eventObj) return;
			// The byte-runtime SignedEvent message carries no request id, so
			// requestId() is 0 and delivery falls back to the oldest pending
			// request (FIFO).
			const cb = this.takeSignCallback(signedEventObj!.requestId() || undefined);
			if (cb) {
				cb(this.fbEventToNostrEvent(eventObj));
			}
			return;
		}
		if (workerMsg.contentType() !== Message.Pubkey) return;
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
	}

	private isPubkeyResult(value: unknown): value is string {
		return typeof value === 'string' && /^[0-9a-f]{64}$/i.test(value);
	}

	private handleSignerPubkey(pubkey: string, secretKey?: unknown, bunkerUrl?: unknown) {
		this.activePubkey = pubkey;
		if (this._pendingSession) {
			const sessionPayload =
				this._pendingSession.type === 'nip46' &&
				typeof bunkerUrl === 'string' &&
				bunkerUrl.startsWith('bunker://') &&
				this._pendingSession.payload &&
				typeof this._pendingSession.payload === 'object'
					? { ...this._pendingSession.payload, url: bunkerUrl }
					: this._pendingSession.payload;
			this.saveSession(pubkey, this._pendingSession.type, sessionPayload);
			this._pendingSession = null;
		}
		this.dispatch('auth', { pubkey: this.activePubkey, hasSigner: true, secretKey });
	}

	private fbEventToNostrEvent(eventObj: FbNostrEvent): NostrEvent {
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
			if (!tag) continue;
			const values: string[] = [];
			for (let j = 0; j < tag.itemsLength(); j++) {
				const value = tag.items(j);
				if (value !== null) values.push(value);
			}
			signedEvent.tags.push(values);
		}
		return signedEvent;
	}

	private handleCryptoMessage(payload: Uint8Array): void {
		const bb = new flatbuffers.ByteBuffer(payload);
		const workerMsg = WorkerMessage.getRootAsWorkerMessage(bb);
		switch (workerMsg.contentType()) {
			case Message.SetSignerResponse: {
				const resp = workerMsg.content(new SetSignerResponse());
				if (!resp) return;
				const pubkey = resp.pubkey() || '';
				const secretKey =
					this._pendingSession?.type === 'privkey' ? this._pendingSession.payload : undefined;
				if (this.isPubkeyResult(pubkey)) {
					this.handleSignerPubkey(pubkey, secretKey, resp.bunkerUrl());
				} else if (resp.error()) {
					this.dispatch('auth', { pubkey: null, hasSigner: false });
				}
				// Otherwise pubkey carries a NIP-46 QR status string
				// ('awaiting discovery') - a second SetSignerResponse with the real
				// pubkey and bunker_url arrives once discovery completes.
				return;
			}
			case Message.Pubkey: {
				const resp = workerMsg.content(new Pubkey());
				const pubkey = resp?.pubkey() || '';
				const secretKey =
					this._pendingSession?.type === 'privkey' ? this._pendingSession.payload : undefined;
				if (this.isPubkeyResult(pubkey)) {
					this.handleSignerPubkey(pubkey, secretKey);
				} else {
					this.dispatch('auth', { pubkey: this.activePubkey, hasSigner: false, secretKey });
				}
				return;
			}
			case Message.SignedEvent: {
				const resp = workerMsg.content(new SignedEvent());
				if (!resp) return;
				const eventObj = resp.event();
				if (!eventObj) {
					if (resp.error()) {
						console.warn('[ReactNativeManager] sign_event failed:', resp.error());
					}
					return;
				}
				const cb = this.takeSignCallback(resp.requestId() || undefined);
				if (cb) {
					cb(this.fbEventToNostrEvent(eventObj));
				}
				return;
			}
			case Message.Raw: {
				// Only emitted for malformed MainMessage payloads.
				const raw = workerMsg.content(new Raw());
				console.warn('[ReactNativeManager] crypto worker error:', raw?.raw());
				return;
			}
		}
	}

	subscribe(
		subscriptionId: string,
		requests: RequestObject[],
		options: SubscriptionConfig
	): ArrayBuffer {
		const subId = subscriptionId;
		const existing = this.nativeModule.retainSubscriptionBuffer(subId);
		if (existing instanceof ArrayBuffer) {
			return existing;
		}
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
			options.pagination ? this.textEncoder.encode(options.pagination) : null,
			options.cacheOnly
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
						r.noCache,
						undefined,
						options.cacheOnly,
						r.meshOnly
					)
			),
			optionsT
		);
		const mainT = new MainMessageT(MainContent.Subscribe, subscribeT);
		const builder = new flatbuffers.Builder(2048);
		builder.finish(mainT.pack(builder));
		const buffer = this.nativeModule.subscribe(builder.asUint8Array(), subId);
		if (!(buffer instanceof ArrayBuffer)) {
			throw new Error('[ReactNativeManager] native subscription buffer unavailable');
		}
		return buffer;
	}

	override getBuffer(subId: string): ArrayBuffer | undefined {
		console.warn(
			`[ReactNativeManager] getBuffer(${subId}) is deprecated. Use subscribe() through useSubscription so Rust can own subscription lifetime.`
		);
		return undefined;
	}

	override unsubscribe(subscriptionId: string): void {
		this.nativeModule.releaseSubscription(subscriptionId);
	}

	publish(
		publish_id: string,
		event: NostrEvent,
		defaultRelays: string[] = [],
		optimisticSubIds?: string[]
	): ArrayBuffer {
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
		const buffer = this.nativeModule.publish(builder.asUint8Array(), publish_id);
		if (!(buffer instanceof ArrayBuffer)) {
			throw new Error(`[ReactNativeManager] Failed to get native publish buffer '${publish_id}'`);
		}
		return buffer;
	}

	releasePublish(publish_id: string): void {
		this.nativeModule.releaseSubscription(publish_id);
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
				this.nativeModule.setPrivateKey(payload as string);
				this.getPublicKey();
				break;
			case 'nip07':
				console.warn('[ReactNativeManager] NIP-07 is not supported in React Native');
				this.dispatch('auth', { pubkey: null, hasSigner: false });
				break;
			case 'nip46': {
				const nip46Payload = payload as { url: string; clientSecret: string } | undefined;
				const url = nip46Payload?.url || '';
				const clientSecret = nip46Payload?.clientSecret;
				const signerT = url.startsWith('bunker://')
					? new SetSignerT(SignerType.Nip46Bunker, new Nip46BunkerT(url, clientSecret))
					: url.startsWith('nostrconnect://')
						? new SetSignerT(SignerType.Nip46QR, new Nip46QRT(url, clientSecret))
						: null;
				if (!signerT) {
					this._pendingSession = null;
					this.dispatch('auth', { pubkey: null, hasSigner: false });
					return;
				}
				const mainT = new MainMessageT(MainContent.SetSigner, signerT);
				const builder = new flatbuffers.Builder(2048);
				builder.finish(mainT.pack(builder));
				this.postMessage(builder.asUint8Array());
				break;
			}
		}
	}

	setMeshProfile(profile: NostrEvent): boolean {
		if (profile.kind !== 0) {
			throw new Error('[ReactNativeManager] Mesh profile must be a signed kind-0 event');
		}
		return this.nativeModule.setMeshProfile(JSON.stringify(profile));
	}

	clearMeshProfile(): boolean {
		return this.nativeModule.clearMeshProfile();
	}

	signEvent(event: EventTemplate, cb: (event: NostrEvent) => void): void {
		const requestId = this._nextSignRequestId++;
		this._signRequests.set(requestId, cb);
		const templateT = new TemplateT(
			event.kind,
			event.created_at,
			this.textEncoder.encode(event.content),
			event.tags.map((t) => new StringVecT(t)) || []
		);
		const signEventT = new SignEventT(templateT, requestId);
		const mainT = new MainMessageT(MainContent.SignEvent, signEventT);
		const builder = new flatbuffers.Builder(2048);
		builder.finish(mainT.pack(builder));
		this.postMessage(builder.asUint8Array());
	}

	/**
	 * Resolve the callback for a sign_event response. Prefers an exact
	 * request-id match; falls back to the oldest pending request for
	 * responses that carry no id (byte runtime, legacy producers).
	 */
	private takeSignCallback(id?: number): ((event: NostrEvent) => void) | undefined {
		if (id !== undefined) {
			const cb = this._signRequests.get(id);
			if (cb) {
				this._signRequests.delete(id);
				return cb;
			}
		}
		const first = this._signRequests.entries().next();
		if (first.done) return undefined;
		this._signRequests.delete(first.value[0]);
		return first.value[1];
	}

	getPublicKey(): void {
		const mainT = new MainMessageT(MainContent.GetPublicKey, new GetPublicKeyT());
		const builder = new flatbuffers.Builder(2048);
		builder.finish(mainT.pack(builder));
		this.postMessage(builder.asUint8Array());
	}

	protected onLogout(): void {}

	cleanup(): void {
		this.nativeModule.cleanupSubscriptions();
	}

	deinit(): void {
		this.deinitialized = true;
		this.appStateSubscription?.remove();
		this.appStateSubscription = undefined;
		if (this.eventListener) {
			this.eventEmitter?.removeListener(REACT_NATIVE_EVENT_NAME, this.eventListener);
			this.eventListener = undefined;
		}
		this.nativeModule.deinit();
	}
}

export { ReactNativeManager as ReactNativeBackend };

export function getOrCreateReactNativeBackend(config: NostrManagerConfig = {}): ReactNativeManager {
	if (reactNativeBackendInstance && !reactNativeBackendInstance.isDeinitialized()) {
		return reactNativeBackendInstance;
	}
	reactNativeBackendInstance = new ReactNativeManager(config);
	return reactNativeBackendInstance;
}

export function createNostrManager(config?: NostrManagerConfig): NostrManagerLike {
	return getOrCreateReactNativeBackend(config);
}

/** Retry starting the platform BLE transport after runtime permissions are granted. */
export function startMeshBLE(): boolean {
	const mod = getReactNativeModule();
	return typeof mod.startMesh === 'function' ? Boolean(mod.startMesh()) : false;
}

export function stopMeshBLE(): void {
	const mod = getReactNativeModule();
	if (typeof mod.stopMesh === 'function') mod.stopMesh();
}

/** Pin a signed kind-0 profile as this device's visible nearby identity. */
export function setMeshProfile(profile: NostrEvent): boolean {
	return getOrCreateReactNativeBackend().setMeshProfile(profile);
}

/** Stop sharing the local profile while continuing to relay mesh events. */
export function clearMeshProfile(): boolean {
	return getOrCreateReactNativeBackend().clearMeshProfile();
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
