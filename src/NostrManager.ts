import * as flatbuffers from 'flatbuffers';
import type { EventTemplate, NostrEvent } from 'nostr-tools';

import { ArrayBufferReader } from 'src/lib/ArrayBufferReader';
import { BaseBackend, localStorageAdapter } from 'src/lib/BaseBackend';

import type { NostrManagerConfig, RequestObject, SubscriptionConfig } from 'src/types';
import type { InitCacheMsg } from './cache/index';
import type { InitConnectionsMsg } from './connections/types';
import type { InitCryptoMsg } from './crypto/index';
import {
	BufferFullT,
	ConnectionStatus,
	GetPublicKeyT,
	MainContent,
	MainMessageT,
	Message,
	MessageType,
	Nip07T,
	Nip46BunkerT,
	Nip46QRT,
	NostrEvent as FbNostrEvent,
	PipelineConfigT,
	PrivateKeyT,
	Pubkey,
	PublishT,
	Raw,
	RequestT,
	SetSignerResponse,
	SetSignerT,
	SignedEvent,
	SignEventT,
	SignerType,
	StringVec,
	StringVecT,
	SubscribeT,
	SubscriptionConfigT,
	TemplateT,
	UnsubscribeT,
	WorkerMessage,
	WorkerMessageT
} from './generated/nostr/fb';
import type { InitParserMsg } from './parser/index';
import { scheduleMicrotask } from './lib/scheduleMicrotask';
import { setManager } from './manager';

// Shared decoder for parser→main frames (avoids per-frame allocation).
const textDecoder = new TextDecoder();

/**
 * NostrManager handles worker orchestration and session persistence.
 */
export class NostrManager extends BaseBackend {
	private connections: Worker;
	private cache: Worker;
	private parser: Worker;
	private crypto: Worker;
	private signRequests = new Map<number, (event: NostrEvent) => void>();
	private nextSignRequestId = 1;
	private lastWakeAt = 0;

	// MessageChannel for parser-main communication
	private parserMainPort: MessagePort;

	// MessageChannel for crypto-main communication
	private cryptoMainPort: MessagePort;

	constructor(config: NostrManagerConfig = {}) {
		super(localStorageAdapter);
		// Create 7 MessageChannels for worker-to-worker communication
		// Each channel connects two workers - each worker gets one port (which is bidirectional)
		// Channel naming: workerA_workerB (no direction, just identifies the pair)
		const parser_cache = new MessageChannel(); // parser ↔ cache
		const parser_connections = new MessageChannel(); // parser ↔ connections
		const parser_crypto = new MessageChannel(); // parser ↔ crypto
		const cache_connections = new MessageChannel(); // cache ↔ connections
		const crypto_connections = new MessageChannel(); // crypto ↔ connections
		const crypto_main = new MessageChannel(); // crypto ↔ main
		const parser_main = new MessageChannel(); // parser ↔ main (for batched events + relay status)

		const useProxyConnections = !!config.proxy;
		// Keep literal paths so Vite can statically rewrite worker URLs in production builds.
		const connectionURL = useProxyConnections
			? new URL('./connections/proxy.js', import.meta.url)
			: new URL('./connections/index.js', import.meta.url);
		const cacheURL = new URL('./cache/index.js', import.meta.url);
		const parserURL = new URL('./parser/index.js', import.meta.url);
		const cryptoURL = new URL('./crypto/index.js', import.meta.url);
		this.connections = new Worker(connectionURL, { type: 'module' });
		this.cache = new Worker(cacheURL, { type: 'module' });
		this.parser = new Worker(parserURL, { type: 'module' });
		this.crypto = new Worker(cryptoURL, { type: 'module' });

		// Transfer ports to connections worker
		// Needs: cachePort, parserPort, cryptoPort
		this.connections.postMessage(
			{
				type: 'init',
				payload: {
					cachePort: cache_connections.port1,
					parserPort: parser_connections.port1,
					cryptoPort: crypto_connections.port1,
					logLevel: config.logLevel,
					...(config.proxy ? { proxy: config.proxy } : {})
				}
			} as InitConnectionsMsg,
			[cache_connections.port1, parser_connections.port1, crypto_connections.port1]
		);

		// Transfer ports to cache worker
		// Needs: parserPort, connectionsPort
		this.cache.postMessage(
			{
				type: 'init',
				payload: {
					parserPort: parser_cache.port1,
					connectionsPort: cache_connections.port2,
					logLevel: config.logLevel,
					defaultRelays: config.defaultRelays,
					indexerRelays: config.indexerRelays
				}
			} as InitCacheMsg,
			[parser_cache.port1, cache_connections.port2]
		);

		// Store parser_main port1 for receiving batched events from parser
		this.parserMainPort = parser_main.port1;

		// Set up message handler for incoming messages from parser
		// Parser worker sends batched frames, concatenated in a single ArrayBuffer:
		// [4 bytes frameLen LE][4 bytes subIdLen LE][subId][WorkerMessage] ...
		this.parserMainPort.onmessage = (event) => {
			const buffer = event.data as ArrayBuffer;
			if (!buffer || !(buffer instanceof ArrayBuffer)) {
				console.log('[main] ignoring message - not ArrayBuffer');
				return;
			}
			const view = new DataView(buffer);
			let offset = 0;
			while (offset + 4 <= buffer.byteLength) {
				const frameLen = view.getUint32(offset, true);
				if (frameLen === 0 || offset + 4 + frameLen > buffer.byteLength) return;
				const frameStart = offset + 4;
				if (frameLen < 4) return;
				const subIdLen = view.getUint32(frameStart, true);
				if (4 + subIdLen > frameLen) return;

				const subId = textDecoder.decode(
					new Uint8Array(buffer, frameStart + 4, subIdLen)
				);
				const data = new Uint8Array(buffer, frameStart + 4 + subIdLen, frameLen - 4 - subIdLen);
				if (data.length > 0) {
					this.handleParserMainFrame(subId, data);
				}
				offset = frameStart + frameLen;
			}
		};

		// Transfer ports to parser worker
		// Needs: connectionsPort, cachePort, cryptoPort, mainPort
		this.parser.postMessage(
			{
				type: 'init',
				payload: {
					connectionsPort: parser_connections.port2,
					cachePort: parser_cache.port2,
					cryptoPort: parser_crypto.port1,
					mainPort: parser_main.port2,
					logLevel: config.logLevel
				}
			} as InitParserMsg,
			[parser_connections.port2, parser_cache.port2, parser_crypto.port1, parser_main.port2]
		);

		// Transfer ports to crypto worker
		// Needs: parserPort, connectionsPort, mainPort
		this.crypto.postMessage(
			{
				type: 'init',
				payload: {
					parserPort: parser_crypto.port2,
					connectionsPort: crypto_connections.port2,
					mainPort: crypto_main.port1,
					logLevel: config.logLevel
				}
			} as InitCryptoMsg,
			[parser_crypto.port2, crypto_connections.port2, crypto_main.port1]
		);

		// Store crypto_main.port2 for sending commands to crypto worker
		this.cryptoMainPort = crypto_main.port2;

		// Listen on crypto_main.port2 for control responses
		crypto_main.port2.onmessage = (event) => {
			const data = event.data;
			if (!(data instanceof ArrayBuffer)) return;
			const bytes = new Uint8Array(data);
			const bb = new flatbuffers.ByteBuffer(bytes);
			const wm = WorkerMessage.getRootAsWorkerMessage(bb);
			this.handleCryptoResponse(wm);
		};

		this.setupWorkerListener();
		this.setupVisibilityTracking();
		// Defer session restore so callers have time to add auth listeners
		scheduleMicrotask(() => this.restoreSession());
		setManager(this);
	}

	/**
	 * Track page visibility changes to handle mobile background/foreground transitions.
	 * Cleanup stale subscriptions when hidden, and wake workers when the page becomes active again.
	 */
	private setupVisibilityTracking(): void {
		if (typeof document === 'undefined' || typeof window === 'undefined') return;

		let wasHidden = false;
		let hiddenTime = 0;
		const wakeFromLifecycle = (source: string) => {
			const hiddenDuration = hiddenTime > 0 ? Date.now() - hiddenTime : 0;
			wasHidden = false;
			hiddenTime = 0;

			console.log(`[main] App returned to foreground after ${hiddenDuration}ms`);
			this.wakeWorkers(source);
		};

		document.addEventListener('visibilitychange', () => {
			if (document.hidden) {
				wasHidden = true;
				hiddenTime = Date.now();
				this.cleanup();
			} else if (wasHidden) {
				wakeFromLifecycle('visibility');
			}
		});

		window.addEventListener('pagehide', () => {
			wasHidden = true;
			hiddenTime = hiddenTime || Date.now();
			this.cleanup();
		});

		window.addEventListener('pageshow', () => {
			if (!document.hidden) {
				wakeFromLifecycle('pageshow');
			}
		});
		window.addEventListener('focus', () => {
			if (!document.hidden) {
				wakeFromLifecycle('focus');
			}
		});
		window.addEventListener('online', () => {
			if (!document.hidden) {
				wakeFromLifecycle('online');
			}
		});
	}

	/**
	 * Send wake signal to all workers to trigger immediate reconnection.
	 * Called when returning from background to foreground.
	 */
	private wakeWorkers(source = 'visibility'): void {
		const now = Date.now();
		if (now - this.lastWakeAt < 250) return;
		this.lastWakeAt = now;
		console.log(`[main] Waking connections worker for foreground reconnection (${source})`);
		this.connections.postMessage({ type: 'wake', source });
	}

	private postToWorker(message: { serializedMessage?: Uint8Array }) {
		const uint8Array = message?.serializedMessage;
		if (uint8Array) {
			this.parserMainPort.postMessage(uint8Array, [uint8Array.buffer]);
		}
	}

	/**
	 * Handle a single parser→main frame: a (subId, WorkerMessage) pair decoded
	 * from the batched wire format. ConnectionStatus frames update relay status;
	 * everything else lands in the subscription/publish ring buffer.
	 */
	private handleParserMainFrame(subId: string, data: Uint8Array): void {
		// Try to parse as WorkerMessage to detect ConnectionStatus
		try {
			const bb = new flatbuffers.ByteBuffer(data);
			const wm = WorkerMessage.getRootAsWorkerMessage(bb);
			if (wm.contentType() === Message.ConnectionStatus) {
				const cs = wm.content(new ConnectionStatus());
				if (cs) {
					const url = cs.relayUrl() || '';
					const status = cs.status() || '';
					if (url && status) {
						this.relayStatuses.set(url, { status, timestamp: Date.now() });
						this.dispatch('relay:status', { status, url });
					}
				}
				// Relay-level statuses have no subscription; subscription-tied
				// statuses (EOSE, OK, etc.) should also flow to the sub buffer.
				if (!subId) {
					return;
				}
			}
		} catch (e) {
			// Not a WorkerMessage, treat as batched event data
		}

		// Regular batched event data
		// Parser worker sends raw FlatBuffer bytes; writePayload prepends the
		// 4-byte LE length directly in the target buffer (no intermediate copy).
		const subscription = this.subscriptions.get(subId);
		if (subscription) {
			const written = ArrayBufferReader.writePayload(subscription.buffer, data, subId);
			if (written) {
				this.dispatch(`subscription:${subId}`, subId);
			} else {
				// The subscription buffer is full: events are being dropped.
				// Best-effort: append a typed BufferFull marker to the buffer so
				// hooks can surface it via isBufferFull (it may not fit - fine),
				// and always notify listeners directly before closing.
				const bufferFullT = new WorkerMessageT(
					this.textEncoder.encode(subId),
					null,
					MessageType.BufferFull,
					Message.BufferFull,
					new BufferFullT(0)
				);
				const bfBuilder = new flatbuffers.Builder(256);
				bfBuilder.finish(bufferFullT.pack(bfBuilder));
				const bfPayload = bfBuilder.asUint8Array();
				if (ArrayBufferReader.writePayload(subscription.buffer, bfPayload, subId)) {
					this.dispatch(`subscription:${subId}`, subId);
				}
				this.dispatch('bufferfull', { subId });
				this.closeSubscription(subId);
			}
			return;
		}

		const publish = this.publishes.get(subId);
		if (publish) {
			const written = ArrayBufferReader.writePayload(publish.buffer, data, subId);
			if (written) {
				this.dispatch(`publish:${subId}`, subId);
			} else {
				this.dispatch('bufferfull', { subId });
				this.publishes.delete(subId);
			}
			return;
		}
	}

	private closeSubscription(subId: string): void {
		const unsubscribeT = new UnsubscribeT(this.textEncoder.encode(subId));
		const mainT = new MainMessageT(MainContent.Unsubscribe, unsubscribeT);
		const builder = new flatbuffers.Builder(256);
		builder.finish(mainT.pack(builder));
		const uint8Array = builder.asUint8Array();
		this.parserMainPort.postMessage(uint8Array, [uint8Array.buffer]);
		this.subscriptions.delete(subId);
	}

	private sendCryptoMessage(contentType: MainContent, content: any) {
		const mainT = new MainMessageT(contentType, content);
		const builder = new flatbuffers.Builder(2048);
		builder.finish(mainT.pack(builder));
		const uint8Array = builder.asUint8Array();
		this.cryptoMainPort.postMessage(uint8Array, [uint8Array.buffer]);
	}

	private isPubkeyResult(value: unknown): value is string {
		return typeof value === 'string' && /^[0-9a-f]{64}$/i.test(value);
	}

	private sessionPayloadForSigner(bunkerUrl?: unknown) {
		if (
			this._pendingSession?.type === 'nip46' &&
			typeof bunkerUrl === 'string' &&
			bunkerUrl.startsWith('bunker://') &&
			this._pendingSession.payload &&
			typeof this._pendingSession.payload === 'object'
		) {
			return { ...this._pendingSession.payload, url: bunkerUrl };
		}
		return this._pendingSession?.payload;
	}

	private handleSignerPubkey(pubkey: string, secretKey?: unknown, bunkerUrl?: unknown) {
		this.activePubkey = pubkey;
		if (this._pendingSession) {
			this.saveSession(
				this.activePubkey,
				this._pendingSession.type,
				this.sessionPayloadForSigner(bunkerUrl)
			);
			this._pendingSession = null;
		}
		this.dispatch('auth', {
			pubkey: this.activePubkey,
			hasSigner: true,
			...(secretKey ? { secretKey } : {})
		});
	}

	private handleCryptoResponse(wm: WorkerMessage) {
		switch (wm.contentType()) {
			case Message.SetSignerResponse: {
				const resp = wm.content(new SetSignerResponse());
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
				const resp = wm.content(new Pubkey());
				const pubkey = resp?.pubkey() || '';
				const secretKey =
					this._pendingSession?.type === 'privkey' ? this._pendingSession.payload : undefined;
				if (this.isPubkeyResult(pubkey)) {
					this.handleSignerPubkey(pubkey, secretKey);
					return;
				}
				this.dispatch('auth', { pubkey: this.activePubkey, hasSigner: false });
				return;
			}
			case Message.SignedEvent: {
				const resp = wm.content(new SignedEvent());
				if (!resp) return;
				const eventObj = resp.event();
				if (!eventObj) {
					if (resp.error()) {
						console.warn('[main] sign_event failed:', resp.error());
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
				const raw = wm.content(new Raw());
				console.warn('[main] crypto worker error:', raw?.raw());
				return;
			}
		}
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

	/**
	 * Resolve the callback for a sign_event response. Prefers an exact
	 * request-id match; falls back to the oldest pending request for
	 * responses that carry no id (legacy/native producers).
	 */
	private takeSignCallback(id?: number): ((event: NostrEvent) => void) | undefined {
		if (id !== undefined) {
			const cb = this.signRequests.get(id);
			if (cb) {
				this.signRequests.delete(id);
				return cb;
			}
		}
		const first = this.signRequests.entries().next();
		if (first.done) return undefined;
		this.signRequests.delete(first.value[0]);
		return first.value[1];
	}

	private setupWorkerListener() {
		// NIP-07 extension requests are handled via crypto worker postMessage
		// (these require main thread access to window.nostr)
		this.crypto.onmessage = async (event) => {
			const msg = event.data;

			// Handle NIP-07 extension requests from the worker
			if (msg?.type === 'extension_request') {
				const { id, op, payload } = msg;
				try {
					const nostr = (window as any).nostr;
					if (!nostr) throw new Error('NIP-07 extension (window.nostr) not found');

					let result;
					switch (op) {
						case 'getPublicKey':
							result = await nostr.getPublicKey();
							break;
						case 'signEvent':
							result = await nostr.signEvent(JSON.parse(payload));
							break;
						case 'nip04Encrypt':
							result = await nostr.nip04.encrypt(payload.pubkey, payload.plaintext);
							break;
						case 'nip04Decrypt':
							result = await nostr.nip04.decrypt(payload.pubkey, payload.ciphertext);
							break;
						case 'nip44Encrypt':
							result = await nostr.nip44.encrypt(payload.pubkey, payload.plaintext);
							break;
						case 'nip44Decrypt':
							result = await nostr.nip44.decrypt(payload.pubkey, payload.ciphertext);
							break;
						default:
							throw new Error(`Unknown extension operation: ${op}`);
					}
					this.crypto.postMessage({ type: 'extension_response', id, ok: true, result });
				} catch (e: any) {
					this.crypto.postMessage({
						type: 'extension_response',
						id,
						ok: false,
						error: e.message || String(e)
					});
				}
				return;
			}
		};
	}

	subscribe(
		subscriptionId: string,
		requests: RequestObject[],
		options: SubscriptionConfig
	): ArrayBuffer {
		const subId = subscriptionId;
		const existingSubscription = this.subscriptions.get(subId);
		if (existingSubscription) {
			existingSubscription.refCount++;
			return existingSubscription.buffer;
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
		const uint8Array = builder.asUint8Array();
		// Transfer the underlying buffer for zero-copy
		this.postToWorker({ serializedMessage: uint8Array });

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
		// Transfer the underlying buffer for zero-copy
		this.postToWorker({ serializedMessage: uint8Array });
		this.publishes.set(publish_id, { buffer });
		return buffer;
	}

	setSigner(name: string, payload?: string | { url: string; clientSecret: string }): void {
		this._pendingSession = { type: name, payload };
		console.log('[main] set pending session:', name);

		switch (name) {
			case 'pubkey':
				this.activePubkey = payload as string;
				this.saveSession(this.activePubkey, 'pubkey', payload);
				this._pendingSession = null;
				this.dispatch('auth', { pubkey: this.activePubkey, hasSigner: false });
				break;
			case 'privkey': {
				const pkT = new PrivateKeyT(payload as string);
				const setSignerT = new SetSignerT(SignerType.PrivateKey, pkT);
				this.sendCryptoMessage(MainContent.SetSigner, setSignerT);
				break;
			}
			case 'nip07': {
				const nip07T = new Nip07T();
				const setSignerT = new SetSignerT(SignerType.Nip07, nip07T);
				this.sendCryptoMessage(MainContent.SetSigner, setSignerT);
				break;
			}
			case 'nip46': {
				const url = (payload as any)?.url || '';
				const clientSecret = (payload as any)?.clientSecret;
				if (url.startsWith('bunker://')) {
					const bunkerT = new Nip46BunkerT(url, clientSecret);
					const setSignerT = new SetSignerT(SignerType.Nip46Bunker, bunkerT);
					this.sendCryptoMessage(MainContent.SetSigner, setSignerT);
				} else if (url.startsWith('nostrconnect://')) {
					const qrT = new Nip46QRT(url, clientSecret);
					const setSignerT = new SetSignerT(SignerType.Nip46QR, qrT);
					this.sendCryptoMessage(MainContent.SetSigner, setSignerT);
				} else {
					console.error('[main] Unknown NIP-46 URL format:', url);
				}
				break;
			}
		}
	}

	signEvent(event: EventTemplate, cb: (event: NostrEvent) => void) {
		const requestId = this.nextSignRequestId++;
		this.signRequests.set(requestId, cb);
		const templateT = new TemplateT(
			event.kind,
			event.created_at,
			this.textEncoder.encode(event.content),
			event.tags.map((t) => new StringVecT(t)) || []
		);
		const signEventT = new SignEventT(templateT, requestId);
		this.sendCryptoMessage(MainContent.SignEvent, signEventT);
	}

	getPublicKey() {
		this.sendCryptoMessage(MainContent.GetPublicKey, new GetPublicKeyT());
	}

	protected onLogout(): void {
		this.crypto.postMessage({ type: 'clear_signer' });
	}

	cleanup(): void {
		const subscriptionsToDelete: string[] = [];

		for (const [subId, subscription] of this.subscriptions.entries()) {
			if (subscription.refCount <= 0 && !this.PERPETUAL_SUBSCRIPTIONS.includes(subId)) {
				subscriptionsToDelete.push(subId);
			}
		}

		// Actually remove and tell workers to drop them
		for (const subId of subscriptionsToDelete) {
			// Send Unsubscribe message to parser
			const unsubscribeT = new UnsubscribeT(this.textEncoder.encode(subId));
			const mainT = new MainMessageT(MainContent.Unsubscribe, unsubscribeT);
			const builder = new flatbuffers.Builder(256);
			builder.finish(mainT.pack(builder));
			const uint8Array = builder.asUint8Array();
			// Transfer the underlying buffer for zero-copy
			this.postToWorker({ serializedMessage: uint8Array });

			// Connections worker subscriptions are closed by parser via Unsubscribe

			// Remove from local subscriptions map
			this.subscriptions.delete(subId);
		}
	}
}
