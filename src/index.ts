import * as flatbuffers from 'flatbuffers';
import type { EventTemplate, NostrEvent } from 'nostr-tools';

import { ArrayBufferReader } from 'src/lib/ArrayBufferReader';

import { RequestObject, SubscriptionConfig } from 'src/types';
import { InitCacheMsg } from './cache/index';
import { InitConnectionsMsg } from './connections/index';
import { InitCryptoMsg } from './crypto/index';
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
import { InitParserMsg } from './parser/index';
export * from './lib/NostrUtils';
export * from './types';

/**
 * NostrManager handles worker orchestration and session persistence.
 */
export class NostrManager {
	private connections: Worker;
	private cache: Worker;
	private parser: Worker;
	private crypto: Worker;
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

	private activePubkey: string | null = null;
	private _pendingSession: { type: string; payload: any } | null = null;

	private signCB = (_event: any) => {};
	private eventTarget = new EventTarget();

	// MessageChannel for parser-main communication
	private parserMainPort: MessagePort;

	// MessageChannel for connections-main communication (relay status)
	private connectionsMainPort: MessagePort;

	// Relay status cache: url -> {status, timestamp}
	private relayStatuses = new Map<
		string,
		{ status: 'connected' | 'failed' | 'close'; timestamp: number }
	>();

	public PERPETUAL_SUBSCRIPTIONS = ['notifications', 'starterpack'];

	constructor() {
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

		const connectionURL = new URL('./connections/index.js', import.meta.url);
		const cacheURL = new URL('./cache/index.js', import.meta.url);
		const parserURL = new URL('./parser/index.js', import.meta.url);
		const cryptoURL = new URL('./crypto/index.js', import.meta.url);

		this.connections = new Worker(connectionURL, { type: 'module' });
		this.cache = new Worker(cacheURL, { type: 'module' });
		this.parser = new Worker(parserURL, { type: 'module' });
		this.crypto = new Worker(cryptoURL, { type: 'module' });

		// Transfer ports to connections worker
		// Needs: mainPort, cachePort, parserPort, cryptoPort
		this.connections.postMessage(
			{
				type: 'init',
				payload: {
					mainPort: connections_main.port2,
					cachePort: cache_connections.port1,
					parserPort: parser_connections.port1,
					cryptoPort: crypto_connections.port1
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
					connectionsPort: cache_connections.port2
				}
			} as InitCacheMsg,
			[parser_cache.port1, cache_connections.port2]
		);

		// Store parser_main port1 for receiving batched events from parser
		this.parserMainPort = parser_main.port1;

		// Set up message handler for incoming batched events from parser
		this.parserMainPort.onmessage = (event) => {
			const { subId, data } = event.data;
			// console.log('[main] parserMainPort received message:', { subId, dataSize: data?.byteLength });
			if (!subId || !data) {
				console.log('[main] ignoring message - missing subId or data');
				return;
			}

			// Find subscription or publish buffer and write data
			// Note: data is already batched with [4-byte len][payload] format
			const subscription = this.subscriptions.get(subId);
			if (subscription) {
				// console.log('[main] found subscription for subId:', subId);
				const written = ArrayBufferReader.writeBatchedData(subscription.buffer, data, subId);
				if (written) {
					this.dispatch(`subscription:${subId}`, subId);
				}
				return;
			}

			const publish = this.publishes.get(subId);
			if (publish) {
				// console.log('[main] found publish for subId:', subId);
				const written = ArrayBufferReader.writeBatchedData(publish.buffer, data, subId);
				if (written) {
					this.dispatch(`publish:${subId}`, subId);
				}
				return;
			}

			console.log('[main] no subscription or publish found for subId:', subId);
		};

		// Set up message handler for relay status from connections worker
		this.connectionsMainPort = connections_main.port1;
		this.connectionsMainPort.onmessage = (event) => {
			const { type, status, url } = JSON.parse(event.data);
			if (type === 'relay:status' && url && status) {
				this.relayStatuses.set(url, { status, timestamp: Date.now() });
				this.dispatch('relay:status', { status, url });
			} else {
				console.log('[main] ignoring message - type:', type, 'url:', url, 'status:', status);
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
					mainPort: parser_main.port2
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
					mainPort: crypto_main.port1
				}
			} as InitCryptoMsg,
			[parser_crypto.port2, crypto_connections.port2, crypto_main.port1]
		);

		// Listen on crypto_main.port2 for control responses
		crypto_main.port2.onmessage = (event) => {
			const msg = event.data;
			if (msg?.type === 'bunker_discovered') {
				const { bunker_url } = msg;
				if (this.activePubkey && this._pendingSession && this._pendingSession.type === 'nip46_qr') {
					const clientSecret = this._pendingSession.payload.clientSecret;
					this.saveSession(this.activePubkey!, 'nip46_bunker', {
						url: bunker_url,
						clientSecret
					});
				}
				return;
			}

			if (msg.type === 'response') {
				this.handleCryptoResponse(msg);
			}
		};

		this.setupWorkerListener();
		this.restoreSession();
	}

	private postToWorker(message: any, transfer?: Transferable[]) {
		if (transfer && transfer.length) {
			this.parser.postMessage(message, transfer);
		} else {
			this.parser.postMessage(message);
		}
	}

	private handleCryptoResponse(msg: any) {
		if (msg.op === 'get_pubkey') {
			if (msg.ok) {
				this.activePubkey = msg.result;
				if (this._pendingSession) {
					this.saveSession(
						this.activePubkey!,
						this._pendingSession.type,
						this._pendingSession.payload
					);
					this._pendingSession = null;
				} else {
					console.log('[main] no pending session to save');
				}
			}
			this.dispatch('auth', this.activePubkey);
		} else if (msg.op === 'sign_event' && msg.ok) {
			const parsed = JSON.parse(msg.result);
			this.signCB(parsed);
		}
	}

	private setupWorkerListener() {
		this.parser.onmessage = async (event) => {
			const id = typeof event.data === 'string' ? event.data : undefined;
			if (!id) return;
			if (this.subscriptions.has(id)) {
				this.dispatch(`subscription:${id}`, id);
				return;
			}
			if (this.publishes.has(id)) {
				this.dispatch(`publish:${id}`, id);
				return;
			}
		};

		// NIP-07 extension requests are still handled via crypto worker postMessage
		// (these require main thread access to window.nostr)
		this.crypto.onmessage = async (event) => {
			const msg = event.data;

			// Handle control responses (get_pubkey, sign_event, etc.)
			if (msg?.type === 'response') {
				this.handleCryptoResponse(msg);
				return;
			}

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
			options.isSlow
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
		// Transfer the underlying buffer for zero-copy
		this.postToWorker({ serializedMessage: uint8Array }, [uint8Array.buffer]);

		return buffer;
	}

	getBuffer(subId: string): ArrayBuffer | undefined {
		const existingSubscription = this.subscriptions.get(subId);
		if (existingSubscription) {
			existingSubscription.refCount++;
			return existingSubscription.buffer;
		}
		return undefined;
	}

	/**
	 * Get current relay statuses. Returns a Map of relay URL to status.
	 * Use this for initial state when mounting useRelayStatus.
	 */
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

	publish(publish_id: string, event: NostrEvent, defaultRelays: string[] = []): ArrayBuffer {
		const buffer = new ArrayBuffer(3072);
		ArrayBufferReader.initializeBuffer(buffer);

		const templateT = new TemplateT(
			event.kind,
			event.created_at,
			this.textEncoder.encode(event.content),
			event.tags.map((t) => new StringVecT(t)) || []
		);
		const publishT = new PublishT(this.textEncoder.encode(publish_id), templateT, defaultRelays);
		const mainT = new MainMessageT(MainContent.Publish, publishT);
		const builder = new flatbuffers.Builder(2048);
		builder.finish(mainT.pack(builder));
		const uint8Array = builder.asUint8Array();
		// Transfer the underlying buffer for zero-copy
		this.postToWorker({ serializedMessage: uint8Array }, [uint8Array.buffer]);
		this.publishes.set(publish_id, { buffer });
		return buffer;
	}

	setSigner(name: string, payload?: string | { url: string; clientSecret: string }): void {
		switch (name) {
			case 'privkey':
				this.crypto.postMessage({ type: 'set_private_key', payload });
				break;
			case 'nip07':
				this.crypto.postMessage({ type: 'set_nip07' });
				break;
			case 'nip46_bunker':
				this.crypto.postMessage({ type: 'set_nip46_bunker', payload });
				break;
			case 'nip46_qr':
				this.crypto.postMessage({ type: 'set_nip46_qr', payload });
				break;
		}

		// Trigger pubkey discovery to validate and save session
		if (name === 'privkey' || name === 'nip07' || name === 'nip46_bunker' || name === 'nip46_qr') {
			this._pendingSession = { type: name, payload };
			console.log('[main] set pending session:', name);
			this.crypto.postMessage({ type: 'get_pubkey' });
		}
	}

	signEvent(event: EventTemplate, cb: (event: NostrEvent) => void) {
		this.signCB = cb;
		this.crypto.postMessage({ type: 'sign_event', payload: JSON.stringify(event) });
	}

	getPublicKey() {
		const mainT = new MainMessageT(MainContent.GetPublicKey, new GetPublicKeyT());
		const builder = new flatbuffers.Builder(2048);
		builder.finish(mainT.pack(builder));
		const uint8Array = builder.asUint8Array();
		// Transfer the underlying buffer for zero-copy
		this.parser.postMessage({ serializedMessage: uint8Array }, [uint8Array.buffer]);
	}

	setNip46Bunker(bunkerUrl: string): void {
		this.setSigner('nip46_bunker', bunkerUrl);
	}

	setNip46QR(nostrconnectUrl: string, clientSecret?: string): void {
		this.setSigner(
			'nip46_qr',
			clientSecret ? { url: nostrconnectUrl, clientSecret } : nostrconnectUrl
		);
	}

	setNip07(): void {
		this.setSigner('nip07', '');
	}

	public getActivePubkey(): string | null {
		return this.activePubkey;
	}

	public getSubscriptionCount(): number {
		return this.subscriptions.size;
	}

	public getAccounts(): Record<string, { type: string; payload: any }> {
		const accountsJson = localStorage.getItem('nostr_signer_accounts') || '{}';
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
		}
	}

	public logout(): void {
		this._pendingSession = null;
		this.activePubkey = null;
		this.crypto.postMessage({ type: 'clear_signer' });
		localStorage.removeItem('nostr_active_pubkey');
		this.dispatch('logout');
	}

	public removeAccount(): void {
		const currentPubkey = this.activePubkey;
		if (!currentPubkey) return;

		// Remove current account from storage
		const accounts = this.getAccounts();
		delete accounts[currentPubkey];
		localStorage.setItem('nostr_signer_accounts', JSON.stringify(accounts));

		// Check for other accounts to switch to
		const remainingPubkeys = Object.keys(accounts);
		if (remainingPubkeys.length > 0) {
			// Switch to first available account
			const nextPubkey = remainingPubkeys[0];
			if (nextPubkey) {
				this.switchAccount(nextPubkey);
			}
		} else {
			// No other accounts - perform full logout
			this.logout();
		}
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
			this.postToWorker({ serializedMessage: uint8Array }, [uint8Array.buffer]);

			// Tell connections worker to close relay subscriptions
			this.connections.postMessage(subId);

			// Remove from local subscriptions map
			this.subscriptions.delete(subId);
		}
	}
}

export const manager = new NostrManager();
