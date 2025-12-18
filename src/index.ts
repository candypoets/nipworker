import * as flatbuffers from 'flatbuffers';
import type { EventTemplate, NostrEvent } from 'nostr-tools';

import { SharedBufferReader } from 'src/lib/SharedBuffer';

import { RequestObject, SubscriptionConfig } from 'src/types';
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
	TemplateT
} from './generated/nostr/fb';
import { InitConnectionsMsg } from './connections';
import { InitCacheMsg } from './cache';
import { InitParserMsg } from './parser';
import { InitSignerMsg } from './signer';
export * from './lib/nostrUtils';

// Idempotent header initializer for rings created on the TS side.
export function initializeRingHeader(size: number): SharedArrayBuffer {
	const buffer = new SharedArrayBuffer(size);
	const HEADER = 32;
	const view = new DataView(buffer);
	const total = buffer.byteLength;

	if (total < HEADER) {
		throw new Error(`Ring buffer too small: ${total} bytes`);
	}

	const cap = view.getUint32(0, true);
	if (cap !== 0) {
		return buffer;
	}
	const capacity = total - HEADER;
	if (capacity <= 0) {
		throw new Error(`Invalid ring capacity computed from total=${total}`);
	}

	// Initialize header: capacity, head=0, tail=0, seq=0, reserved=0
	view.setUint32(0, capacity, true); // capacity
	view.setUint32(4, 0, true); // head
	view.setUint32(8, 0, true); // tail
	view.setUint32(12, 0, true); // seq
	for (let off = 16; off < 32; off += 4) {
		view.setUint32(off, 0, true);
	}
	return buffer;
}

export const statusRing = initializeRingHeader(512 * 1024);

/**
 * NostrManager handles worker orchestration and session persistence.
 */
export class NostrManager {
	private connections: Worker;
	private cache: Worker;
	private parser: Worker;
	private signer: Worker;
	private textEncoder = new TextEncoder();
	private subscriptions = new Map<
		string,
		{
			buffer: SharedArrayBuffer;
			options: SubscriptionConfig;
			refCount: number;
		}
	>();
	private publishes = new Map<string, { buffer: SharedArrayBuffer }>();
	private signers = new Map<string, string>(); // name -> last payload

	private activePubkey: string | null = null;
	private _pendingSession: { type: string; payload: any } | null = null;

	private signCB = (event: any) => {};
	private eventTarget = new EventTarget();

	public PERPETUAL_SUBSCRIPTIONS = ['notifications', 'starterpack'];

	constructor() {
		const wsRequest = initializeRingHeader(1 * 1024 * 1024);
		const wsResponse = initializeRingHeader(5 * 1024 * 1024);

		const wsSignerRequest = initializeRingHeader(512 * 1024);
		const wsSignerResponse = initializeRingHeader(512 * 1024);

		const cacheRequest = initializeRingHeader(1 * 1024 * 1024);
		const cacheResponse = initializeRingHeader(10 * 1024 * 1024);

		const dbRing = initializeRingHeader(2 * 1024 * 1024);

		const signerRequest = initializeRingHeader(512 * 1024);
		const signerResponse = initializeRingHeader(512 * 1024);

		const connectionURL = new URL('./connections/index.js', import.meta.url);
		const cacheURL = new URL('./cache/index.js', import.meta.url);
		const parserURL = new URL('./parser/index.js', import.meta.url);
		const signerURL = new URL('./signer/index.js', import.meta.url);

		this.connections = new Worker(connectionURL, { type: 'module' });
		this.cache = new Worker(cacheURL, { type: 'module' });
		this.parser = new Worker(parserURL, { type: 'module' });
		this.signer = new Worker(signerURL, { type: 'module' });

		this.connections.postMessage({
			type: 'init',
			payload: {
				ws_request: wsRequest,
				ws_response: wsResponse,
				statusRing,
				ws_signer_request: wsSignerRequest,
				ws_signer_response: wsSignerResponse
			}
		} as InitConnectionsMsg);

		this.cache.postMessage({
			type: 'init',
			payload: {
				cache_request: cacheRequest,
				cache_response: cacheResponse,
				ws_request: wsRequest,
				ingestRing: dbRing
			}
		} as InitCacheMsg);

		this.parser.postMessage({
			type: 'init',
			payload: {
				ingestRing: dbRing,
				cacheRequest,
				cacheResponse,
				signerRequest,
				signerResponse,
				wsResponse
			}
		} as InitParserMsg);

		this.signer.postMessage({
			type: 'init',
			payload: { signerRequest, signerResponse, wsSignerRequest, wsSignerResponse }
		} as InitSignerMsg);

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

		this.signer.onmessage = async (event) => {
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
					this.signer.postMessage({ type: 'extension_response', id, ok: true, result });
				} catch (e: any) {
					this.signer.postMessage({
						type: 'extension_response',
						id,
						ok: false,
						error: e.message || String(e)
					});
				}
				return;
			}

			// Handle standard responses
			if (msg.type === 'response') {
				if (msg.op === 'get_pubkey' && msg.ok) {
					this.activePubkey = msg.result;
					if (this._pendingSession) {
						this.saveSession(
							this.activePubkey!,
							this._pendingSession.type,
							this._pendingSession.payload
						);
						this._pendingSession = null;
					}
					this.dispatch('auth', this.activePubkey);
				} else if (msg.op === 'sign_event' && msg.ok) {
					const parsed = JSON.parse(msg.result);
					this.signCB(parsed);
				}
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
	): SharedArrayBuffer {
		const subId = this.createShortId(subscriptionId);
		const existingSubscription = this.subscriptions.get(subId);
		if (existingSubscription) {
			existingSubscription.refCount++;
			return existingSubscription.buffer;
		}

		const totalLimit = requests.reduce((sum, req) => sum + (req.limit || 100), 0);
		const bufferSize = SharedBufferReader.calculateBufferSize(totalLimit, options.bytesPerEvent);
		const buffer = new SharedArrayBuffer(bufferSize);
		SharedBufferReader.initializeBuffer(buffer);

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
		this.postToWorker({ serializedMessage: builder.asUint8Array(), sharedBuffer: buffer });

		return buffer;
	}

	getBuffer(subId: string): SharedArrayBuffer | undefined {
		const existingSubscription = this.subscriptions.get(subId);
		if (existingSubscription) {
			existingSubscription.refCount++;
			return existingSubscription.buffer;
		}
		return undefined;
	}

	unsubscribe(subscriptionId: string): void {
		const subId = subscriptionId.length < 64 ? subscriptionId : this.createShortId(subscriptionId);
		const subscription = this.subscriptions.get(subId);
		if (subscription) {
			subscription.refCount--;
		}
	}

	publish(publish_id: string, event: NostrEvent, defaultRelays: string[] = []): SharedArrayBuffer {
		const buffer = new SharedArrayBuffer(3072);
		SharedBufferReader.initializeBuffer(buffer);

		try {
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
			this.postToWorker({ serializedMessage: builder.asUint8Array(), sharedBuffer: buffer });
			this.publishes.set(publish_id, { buffer });
			return buffer;
		} catch (error) {
			console.error('Failed to publish event:', error);
			throw error;
		}
	}

	setSigner(name: string, payload?: string | { url: string; clientSecret: string }): void {
		console.log('Setting signer:', name, payload);
		switch (name) {
			case 'privkey':
				this.signer.postMessage({ type: 'set_private_key', payload });
				break;
			case 'nip07':
				this.signer.postMessage({ type: 'set_nip07' });
				break;
			case 'nip46_bunker':
				this.signer.postMessage({ type: 'set_nip46_bunker', payload });
				break;
			case 'nip46_qr':
				this.signer.postMessage({ type: 'set_nip46_qr', payload });
				break;
		}

		this.signers.set(name, typeof payload === 'string' ? payload : payload.url);

		// Trigger pubkey discovery to validate and save session
		if (name === 'privkey' || name === 'nip07' || name === 'nip46_bunker') {
			this._pendingSession = { type: name, payload };
			this.signer.postMessage({ type: 'get_pubkey' });
		}
	}

	signEvent(event: EventTemplate, cb: (event: NostrEvent) => void) {
		this.signCB = cb;
		this.signer.postMessage({ type: 'sign_event', payload: event });
	}

	connect() {
		this.signer.postMessage({ type: 'connect' });
	}

	getPublicKey() {
		const mainT = new MainMessageT(MainContent.GetPublicKey, new GetPublicKeyT());
		const builder = new flatbuffers.Builder(2048);
		builder.finish(mainT.pack(builder));
		this.parser.postMessage(builder.asUint8Array());
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

	public logout() {
		this.activePubkey = null;
		localStorage.removeItem('nostr_active_pubkey');
		this.dispatch('logout');
	}

	cleanup(): void {
		const subscriptionsToDelete: string[] = [];
		for (const [subId, subscription] of this.subscriptions.entries()) {
			if (subscription.refCount <= 0 && !this.PERPETUAL_SUBSCRIPTIONS.includes(subId)) {
				subscriptionsToDelete.push(subId);
			}
		}
	}
}

export const manager = new NostrManager();
export * from './types';
