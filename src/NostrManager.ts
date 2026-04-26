import * as flatbuffers from 'flatbuffers';
import type { EventTemplate, NostrEvent } from 'nostr-tools';

import { ArrayBufferReader } from 'src/lib/ArrayBufferReader';
import { BaseBackend, localStorageAdapter } from 'src/lib/BaseBackend';

import type { NostrManagerConfig, RequestObject, SubscriptionConfig } from 'src/types';
import type { InitCacheMsg } from './cache/index';
import type { InitConnectionsMsg } from './connections/types';
import type { InitCryptoMsg } from './crypto/index';
import {
	ConnectionStatus,
	GetPublicKeyT,
	MainContent,
	MainMessageT,
	MessageType,
	Nip07T,
	Nip46BunkerT,
	Nip46QRT,
	PipelineConfigT,
	PrivateKeyT,
	PublishT,
	Raw,
	RequestT,
	SetSignerT,
	SignEventT,
	SignerType,
	StringVecT,
	SubscribeT,
	SubscriptionConfigT,
	TemplateT,
	UnsubscribeT,
	WorkerMessage
} from './generated/nostr/fb';
import type { InitParserMsg } from './parser/index';

/**
 * NostrManager handles worker orchestration and session persistence.
 */
export class NostrManager extends BaseBackend {
	private connections: Worker;
	private cache: Worker;
	private parser: Worker;
	private crypto: Worker;
	private signCB = (_event: any) => {};

	// MessageChannel for parser-main communication
	private parserMainPort: MessagePort;

	// MessageChannel for connections-main communication (relay status)
	private connectionsMainPort: MessagePort;

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
		const parser_main = new MessageChannel(); // parser ↔ main (for batched events)
		const connections_main = new MessageChannel(); // connections ↔ main (for relay status)

		const useProxyConnections = !!config.proxy;
		// Keep literal paths so Vite can statically rewrite worker URLs in production builds.
		const connectionURL = useProxyConnections
			? new URL('./connections/proxy.ts', import.meta.url)
			: new URL('./connections/index.ts', import.meta.url);
		const cacheURL = new URL('./cache/index.ts', import.meta.url);
		const parserURL = new URL('./parser/index.ts', import.meta.url);
		const cryptoURL = new URL('./crypto/index.ts', import.meta.url);
		console.log('constructing crates');
		this.connections = new Worker(connectionURL, { type: 'module' });
		this.cache = new Worker(cacheURL, { type: 'module' });
		this.parser = new Worker(parserURL, { type: 'module' });
		this.crypto = new Worker(cryptoURL, { type: 'module' });

		console.log('connectionMode', useProxyConnections ? 'proxy' : 'rust');

		console.log(this.connections, this.cache, this.parser, this.crypto);
		// Transfer ports to connections worker
		// Needs: mainPort, cachePort, parserPort, cryptoPort
		this.connections.postMessage(
			{
				type: 'init',
				payload: {
					mainPort: connections_main.port2,
					cachePort: cache_connections.port1,
					parserPort: parser_connections.port1,
					cryptoPort: crypto_connections.port1,
					logLevel: config.logLevel,
					...(config.proxy ? { proxy: config.proxy } : {})
				}
			} as InitConnectionsMsg,
			[
				connections_main.port2,
				cache_connections.port1,
				parser_connections.port1,
				crypto_connections.port1
			]
		);

		// Transfer ports to cache worker
		// Needs: parserPort, connectionsPort
		this.cache.postMessage(
			{
				type: 'init',
				payload: {
					parserPort: parser_cache.port1,
					connectionsPort: cache_connections.port2,
					logLevel: config.logLevel
				}
			} as InitCacheMsg,
			[parser_cache.port1, cache_connections.port2]
		);

		// Store parser_main port1 for receiving batched events from parser
		this.parserMainPort = parser_main.port1;

		// Set up message handler for incoming messages from parser
		// Core parser sends tagged bytes: [4 bytes subIdLen LE][subId][data]
		this.parserMainPort.onmessage = (event) => {
			const buffer = event.data as ArrayBuffer;
			if (!buffer || !(buffer instanceof ArrayBuffer)) {
				console.log('[main] ignoring message - not ArrayBuffer');
				return;
			}
			const view = new DataView(buffer);
			if (buffer.byteLength < 4) return;
			const subIdLen = view.getUint32(0, true);
			if (buffer.byteLength < 4 + subIdLen) return;

			const subId = new TextDecoder().decode(new Uint8Array(buffer, 4, subIdLen));
			const data = new Uint8Array(buffer, 4 + subIdLen);
			if (data.length === 0) return;

			// Try to parse as WorkerMessage to detect ConnectionStatus
			try {
				const bb = new flatbuffers.ByteBuffer(data);
				const wm = WorkerMessage.getRootAsWorkerMessage(bb);
				if (wm.type() === MessageType.ConnectionStatus) {
					const cs = wm.content(new ConnectionStatus());
					if (cs) {
						const url = cs.relayUrl() || '';
						const status = cs.status() || '';
						console.log('[main] ConnectionStatus:', status, url);
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
			// Parser worker sends raw FlatBuffer bytes; prepend 4-byte LE length
			// so ArrayBufferReader.readMessages can parse them correctly.
			const lengthPrefixed = new Uint8Array(4 + data.length);
			const lpView = new DataView(lengthPrefixed.buffer);
			lpView.setUint32(0, data.length, true);
			lengthPrefixed.set(data, 4);

			const subscription = this.subscriptions.get(subId);
			if (subscription) {
				const written = ArrayBufferReader.writeBatchedData(subscription.buffer, lengthPrefixed, subId);
				if (written) {
					this.dispatch(`subscription:${subId}`, subId);
				}
				return;
			}

			const publish = this.publishes.get(subId);
			if (publish) {
				const written = ArrayBufferReader.writeBatchedData(publish.buffer, lengthPrefixed, subId);
				if (written) {
					this.dispatch(`publish:${subId}`, subId);
				}
				return;
			}

			console.log('[main] no subscription or publish found for subId:', subId);
		};

		// connectionsMainPort is no longer used for relay status in the new architecture
		// (status comes through parserMainPort as ConnectionStatus WorkerMessages)
		this.connectionsMainPort = connections_main.port1;
		this.connectionsMainPort.onmessage = () => {
			// no-op
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
			if (wm.type() !== MessageType.Raw) return;
			const raw = wm.content(new Raw());
			if (!raw) return;
			const jsonStr = raw.raw();
			if (!jsonStr) return;
			try {
				const msg = JSON.parse(jsonStr);
				if (msg.op) {
					this.handleCryptoResponse(msg);
				}
			} catch (e) {
				console.warn('[main] Failed to parse crypto Raw message:', jsonStr);
			}
		};

		this.setupWorkerListener();
		this.setupVisibilityTracking();
		// Defer session restore so callers have time to add auth listeners
		queueMicrotask(() => this.restoreSession());
	}

	/**
	 * Track page visibility changes to handle mobile background/foreground transitions.
	 * When the app returns to foreground, wake up workers to trigger immediate reconnection.
	 */
	private setupVisibilityTracking(): void {
		if (typeof document === 'undefined') return;

		let wasHidden = false;
		let hiddenTime = 0;

		document.addEventListener('visibilitychange', () => {
			if (document.hidden) {
				wasHidden = true;
				hiddenTime = Date.now();
			} else if (wasHidden) {
				// App is returning to foreground
				const hiddenDuration = Date.now() - hiddenTime;
				wasHidden = false;

				console.log(`[main] App returned to foreground after ${hiddenDuration}ms`);

				// Send wake signal to all workers to trigger immediate reconnection
				// This bypasses the normal reconnect cooldown for better UX
				this.wakeWorkers();
			}
		});
	}

	/**
	 * Send wake signal to all workers to trigger immediate reconnection.
	 * Called when returning from background to foreground.
	 */
	private wakeWorkers(): void {
		console.log('[main] Waking workers for foreground reconnection (no-op in new architecture)');
	}

	private postToWorker(message: { serializedMessage?: Uint8Array }) {
		const uint8Array = message?.serializedMessage;
		if (uint8Array) {
			this.parserMainPort.postMessage(uint8Array, [uint8Array.buffer]);
		}
	}

	private sendCryptoMessage(contentType: MainContent, content: any) {
		const mainT = new MainMessageT(contentType, content);
		const builder = new flatbuffers.Builder(2048);
		builder.finish(mainT.pack(builder));
		const uint8Array = builder.asUint8Array();
		this.cryptoMainPort.postMessage(uint8Array, [uint8Array.buffer]);
	}

	private handleCryptoResponse(msg: any) {
		if (msg.op === 'get_public_key') {
			console.log('[main] get_public_key:', msg.result ? 'success' : 'failed', msg.result);
			if (msg.result) {
				this.activePubkey = msg.result;
			}
			this.dispatch('auth', { pubkey: this.activePubkey, hasSigner: !!msg.result });
		} else if (msg.op === 'set_signer') {
			console.log('[main] set_signer:', msg.result ? 'success' : 'failed', msg.result);
			if (msg.result) {
				this.activePubkey = msg.result;
				if (
					this._pendingSession?.type === 'nip46' &&
					this._pendingSession?.payload?.clientSecret
				) {
					console.log('[main] NIP-46 session saved for:', this.activePubkey);
					this.saveSession(this.activePubkey!, 'nip46', {
						url: this._pendingSession.payload.url,
						clientSecret: this._pendingSession.payload.clientSecret
					});
					this._pendingSession = null;
				} else if (this._pendingSession) {
					const secretKey =
						this._pendingSession?.type === 'privkey'
							? this._pendingSession.payload
							: undefined;
					this.saveSession(
						this.activePubkey!,
						this._pendingSession.type,
						this._pendingSession.payload
					);
					this._pendingSession = null;
					this.dispatch('auth', {
						pubkey: this.activePubkey,
						hasSigner: true,
						secretKey
					});
					return;
				}
				this.dispatch('auth', { pubkey: this.activePubkey, hasSigner: true });
			}
		} else if (msg.op === 'sign_event' && msg.result) {
			const parsed = JSON.parse(msg.result);
			this.signCB(parsed);
		}
	}

	private setupWorkerListener() {
		// NIP-07 extension requests are handled via crypto worker postMessage
		// (these require main thread access to window.nostr)
		this.crypto.onmessage = async (event) => {
			const msg = event.data;
			console.log('[main] crypto.onmessage:', msg?.type, msg);

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
		const subId = this.createShortId(subscriptionId);
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
						this.textEncoder.encode(r.search),
						r.relays,
						r.closeOnEOSE,
						r.cacheFirst,
						r.noCache,
							undefined,
							options.cacheOnly
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
		this.signCB = cb;
		const templateT = new TemplateT(
			event.kind,
			event.created_at,
			this.textEncoder.encode(event.content),
			event.tags.map((t) => new StringVecT(t)) || []
		);
		const signEventT = new SignEventT(templateT);
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
