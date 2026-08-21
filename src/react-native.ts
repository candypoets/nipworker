/**
 * React Native entry point for @candypoets/nipworker.
 *
 * This module exports a ReactNativeManager wired to a React Native native module.
 * It contains no WASM imports and is intended to be consumed as:
 *
 *   import { createNostrManager } from '@candypoets/nipworker/react-native';
 */

import { AppState, type AppStateStatus } from 'react-native';
import * as flatbuffers from 'flatbuffers';

import { BaseBackend, type StorageAdapter } from './lib/BaseBackend';
import { getManager, setManager, setGlobalManager } from './manager';
import type { NostrManagerLike } from './manager';
import type { NostrManagerConfig, RequestObject, SubscriptionConfig } from './types';
import type { EventTemplate, NostrEvent } from 'nostr-tools';
import {
	AuthUrl,
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

let reactNativeBackendInstance: ReactNativeManager | undefined;

type ByteRuntime = {
	init(config?: NostrManagerConfig): void;
	handleMessage(bytes: ArrayBuffer): void;
	wake(): void;
	setPrivateKey(secret: string): void;
	clearSigner(): void;
	removeSigner(): void;
	deinit(): void;
	setWakeHandler(handler?: () => void): void;
	drainPending(): {
		routes: string[];
		packets: ArrayBuffer[];
	};
	getDeliveryStats(): Record<string, number>;
	subscribe(bytes: ArrayBuffer, subId: string): ArrayBuffer | undefined;
	publish(bytes: ArrayBuffer, publishId: string): ArrayBuffer | undefined;
	registerSubscription(subId: string, bufferSize: number): boolean;
	registerPublishBuffer(publishId: string, bufferSize: number): boolean;
	retainSubscriptionBuffer(subId: string): ArrayBuffer | undefined;
	retainSubscription(subId: string): boolean;
	releaseSubscription(subId: string): void;
	getSubscriptionBuffer(subId: string): ArrayBuffer | undefined;
	cleanupSubscriptions(): void;
};

type ReactNativeModuleFacade = {
	init(config?: NostrManagerConfig): void;
	handleMessage(bytes: Uint8Array | ArrayBuffer): void;
	wake(): void;
	setPrivateKey(secret: string): void;
	clearSigner(): void;
	removeSigner(): void;
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

function toExactArrayBuffer(bytes: Uint8Array | ArrayBuffer): ArrayBuffer {
	if (bytes instanceof ArrayBuffer) return bytes;
	if (
		bytes.byteOffset === 0 &&
		bytes.byteLength === bytes.buffer.byteLength &&
		bytes.buffer instanceof ArrayBuffer
	) {
		return bytes.buffer;
	}
	return bytes.slice().buffer;
}

function getByteRuntime(): ByteRuntime | undefined {
	return (globalThis as any).__nipworkerReactNativeByteRuntime;
}

function requireByteRuntime(): ByteRuntime {
	const runtime = getByteRuntime();
	if (!runtime) {
		throw new Error(
			'[ReactNativeBackend] Native delivery transport unavailable. Rebuild the app with the matching nipworker native module.'
		);
	}
	return runtime;
}

function getTurboModule(): any {
	return NativeNipworkerReactNative;
}

function getAnyReactNativeModule(): any {
	return getTurboModule();
}

const reactNativeStorageAdapter: StorageAdapter = {
	getItem(key: string): string | null {
		const value = getReactNativeModule().getStorageItem(key);
		return typeof value === 'string' ? value : null;
	},
	setItem(key: string, value: string): void {
		getReactNativeModule().setStorageItem(key, value);
	},
	removeItem(key: string): void {
		getReactNativeModule().removeStorageItem(key);
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
	storage: reactNativeStorageAdapter,
	getModule(): ReactNativeModuleFacade {
		const mod = getReactNativeModule();
		return {
			init(config?: NostrManagerConfig): void {
				const relayConfig = {
					defaultRelays: config?.defaultRelays ?? [],
					indexerRelays: config?.indexerRelays ?? [],
					meshBLEEnabled: config?.meshBLEEnabled ?? false,
					logLevel: config?.logLevel ?? 'warn'
				};
				// This blocking TurboModule method executes while React Native owns the
				// JS runtime. It must complete before the asynchronous engine method can
				// start Rust workers and produce native deliveries.
				if (!mod.installByteRuntime()) {
					throw new Error('[ReactNativeBackend] Failed to install native delivery transport.');
				}
				mod.initEngine(
					relayConfig.defaultRelays,
					relayConfig.indexerRelays,
					relayConfig.meshBLEEnabled,
					relayConfig.logLevel
				);
				if (relayConfig.meshBLEEnabled) {
					mod.startMesh();
				}
				requireByteRuntime().init(relayConfig);
			},
			handleMessage(bytes: Uint8Array | ArrayBuffer): void {
				requireByteRuntime().handleMessage(toExactArrayBuffer(bytes));
			},
			subscribe(bytes: Uint8Array | ArrayBuffer, subId: string): ArrayBuffer | undefined {
				return requireByteRuntime().subscribe(toExactArrayBuffer(bytes), subId);
			},
			publish(bytes: Uint8Array | ArrayBuffer, publishId: string): ArrayBuffer | undefined {
				return requireByteRuntime().publish(toExactArrayBuffer(bytes), publishId);
			},
			wake(): void {
				requireByteRuntime().wake();
			},
			setPrivateKey(secret: string): void {
				requireByteRuntime().setPrivateKey(secret);
			},
			clearSigner(): void {
				requireByteRuntime().clearSigner();
			},
			removeSigner(): void {
				requireByteRuntime().removeSigner();
			},
			setMeshProfile(profileJson: string): boolean {
				return typeof mod.setMeshProfile === 'function' && Boolean(mod.setMeshProfile(profileJson));
			},
			clearMeshProfile(): boolean {
				return typeof mod.clearMeshProfile === 'function' && Boolean(mod.clearMeshProfile());
			},
			deinit(): void {
				mod.stopMesh();
				requireByteRuntime().deinit();
				mod.deinitEngine();
			},
			registerSubscription(subId: string, bufferSize: number): boolean {
				return requireByteRuntime().registerSubscription(subId, bufferSize);
			},
			registerPublishBuffer(publishId: string, bufferSize: number): boolean {
				return requireByteRuntime().registerPublishBuffer(publishId, bufferSize);
			},
			retainSubscriptionBuffer(subId: string): ArrayBuffer | undefined {
				return requireByteRuntime().retainSubscriptionBuffer(subId);
			},
			retainSubscription(subId: string): boolean {
				return requireByteRuntime().retainSubscription(subId);
			},
			releaseSubscription(subId: string): void {
				requireByteRuntime().releaseSubscription(subId);
			},
			getSubscriptionBuffer(subId: string): ArrayBuffer | undefined {
				return requireByteRuntime().getSubscriptionBuffer(subId);
			},
			cleanupSubscriptions(): void {
				requireByteRuntime().cleanupSubscriptions();
			}
		};
	}
};

export class ReactNativeManager extends BaseBackend {
	private appStateSubscription: { remove: () => void } | undefined;
	private appState = AppState.currentState;
	private nativeModule: ReactNativeModuleFacade;
	private nativeWakeHandler: (() => void) | undefined;
	private deinitialized = false;
	private _signRequests = new Map<number, (event: NostrEvent) => void>();
	private _nextSignRequestId = 1;

	constructor(config: NostrManagerConfig = {}) {
		super(reactNativeStorageAdapter);
		this.nativeModule = reactNativeBridge.getModule();
		this.nativeModule.init(config);
		const byteRuntime = requireByteRuntime();
		this.nativeWakeHandler = () => this.drainNativePending();
		byteRuntime.setWakeHandler(this.nativeWakeHandler);
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

	private drainNativePending(): void {
		if (this.deinitialized) return;
		const pending = requireByteRuntime().drainPending();
		for (const subId of pending.routes) {
			this.dispatch(`subscription:${subId}`, subId);
			this.dispatch(`publish:${subId}`, subId);
		}
		for (const packet of pending.packets) {
			this.handleNativePayload(new Uint8Array(packet));
		}
	}

	private postMessage(bytes: Uint8Array): void {
		this.nativeModule.handleMessage(toExactArrayBuffer(bytes));
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
			if (
				contentType === Message.SetSignerResponse ||
				contentType === Message.Raw ||
				contentType === Message.AuthUrl
			) {
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
		const maybeLenPrefixed = payload.length === 4 + msgLen && msgLen > 0;
		const bb = new flatbuffers.ByteBuffer(
			maybeLenPrefixed ? payload.subarray(4, 4 + msgLen) : payload
		);
		const workerMsg = WorkerMessage.getRootAsWorkerMessage(bb);
		if (workerMsg.contentType() === Message.SignedEvent) {
			const signedEventObj = workerMsg.content(new SignedEvent());
			const eventObj = signedEventObj ? signedEventObj.event() : null;
			if (!eventObj) return;
			// Legacy producers may omit the request id. Delivery is allowed only
			// when exactly one request is pending.
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
			const secretKey =
				this._pendingSession?.type === 'privkey' ? this._pendingSession.payload : undefined;
			this.handleSignerPubkey(pubkey, secretKey);
		} else if (this.canAcceptSignerResponse()) {
			this.dispatch('auth', { pubkey: null, hasSigner: false });
		}
	}

	private isPubkeyResult(value: unknown): value is string {
		return typeof value === 'string' && /^[0-9a-f]{64}$/i.test(value);
	}

	private canAcceptSignerResponse(): boolean {
		return this._pendingSession !== null || this.activePubkey !== null;
	}

	private handleSignerPubkey(pubkey: string, secretKey?: unknown, bunkerUrl?: unknown) {
		if (!this.canAcceptSignerResponse()) return;
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
				if (!this.canAcceptSignerResponse()) return;
				const pubkey = resp.pubkey() || '';
				const secretKey =
					this._pendingSession?.type === 'privkey' ? this._pendingSession.payload : undefined;
				if (this.isPubkeyResult(pubkey)) {
					this.handleSignerPubkey(pubkey, secretKey, resp.bunkerUrl());
				} else if (resp.error()) {
					// Surface the failure instead of failing silently — listeners
					// (e.g. login UI) need the reason (timeout, rejection, …).
					this.dispatch('auth', { pubkey: null, hasSigner: false, error: resp.error() });
				}
				// Otherwise pubkey carries a NIP-46 QR status string
				// ('awaiting discovery') - a second SetSignerResponse with the real
				// pubkey and bunker_url arrives once discovery completes.
				return;
			}
			case Message.Pubkey: {
				const resp = workerMsg.content(new Pubkey());
				if (!this.canAcceptSignerResponse()) return;
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
					this.takeSignCallback(resp.requestId() || undefined);
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
			case Message.AuthUrl: {
				// NIP-46 auth challenge: the app should open the URL so the user
				// can authorize the request; the real response arrives later
				// reusing the same request id.
				const resp = workerMsg.content(new AuthUrl());
				if (!resp) return;
				this.dispatch('authUrl', { url: resp.url() ?? '', requestId: resp.requestId() ?? '' });
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
			true, // Legacy wire field; cache policy is carried by each request.
			options.timeoutMs ? BigInt(options.timeoutMs) : undefined,
			options.maxEvents,
			false, // Legacy wire field; use RequestObject.noCache instead.
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
						r.cacheFirst,
						r.noCache,
						r.maxRelays,
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
		// Responses from a previous account must never cross an account boundary.
		this._signRequests.clear();
		this._pendingSession = { type: name, payload };
		switch (name) {
			case 'pubkey':
				this.nativeModule.clearSigner();
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
				this._pendingSession = null;
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

	override setMeshProfile(profile: NostrEvent): boolean {
		if (profile.kind !== 0) {
			throw new Error('[ReactNativeManager] Mesh profile must be a signed kind-0 event');
		}
		return this.nativeModule.setMeshProfile(JSON.stringify(profile));
	}

	override clearMeshProfile(): boolean {
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
	 * request-id match; accepts a legacy response with no id only when one
	 * request is pending, so it cannot cross an account/request boundary.
	 */
	private takeSignCallback(id?: number): ((event: NostrEvent) => void) | undefined {
		if (id !== undefined) {
			const cb = this._signRequests.get(id);
			if (cb) {
				this._signRequests.delete(id);
				return cb;
			}
			return undefined;
		}
		// A response without an id is safe only when there is no ambiguity.
		if (this._signRequests.size !== 1) return undefined;
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

	protected onLogout(): void {
		this._signRequests.clear();
		this.nativeModule.clearSigner();
	}

	public override removeAccount(): void {
		const currentPubkey = this.activePubkey;
		const session = currentPubkey ? this.getAccounts()[currentPubkey] : undefined;
		if (session?.type === 'nip46') {
			this._signRequests.clear();
			this.nativeModule.removeSigner();
		}
		super.removeAccount();
	}

	cleanup(): void {
		this.nativeModule.cleanupSubscriptions();
	}

	deinit(): void {
		this.deinitialized = true;
		this.appStateSubscription?.remove();
		this.appStateSubscription = undefined;
		if (this.nativeWakeHandler) {
			requireByteRuntime().setWakeHandler(undefined);
			this.nativeWakeHandler = undefined;
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

/** Snapshot counters from the runtime-scoped native delivery transport. */
export function getReactNativeDeliveryStats(): Readonly<Record<string, number>> | null {
	return getByteRuntime()?.getDeliveryStats?.() ?? null;
}

export function hasNativeModule(): boolean {
	return hasReactNativeModule();
}

export { getManager, setManager, setGlobalManager };
export type { NostrManagerLike };
export type * from './types';
