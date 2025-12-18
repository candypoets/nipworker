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
// If capacity (u32 at offset 0) is 0, we set it to (byteLength - 32)
// and zero head, tail, and seq. Reserved bytes are cleared as well.
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
		// Already initialized; nothing to do.
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
	// Zero reserved [16..32)
	for (let off = 16; off < 32; off += 4) {
		view.setUint32(off, 0, true);
	}
	return buffer;
}

export const statusRing = initializeRingHeader(512 * 1024);

/**
 * Pure TypeScript NostrClient that manages worker communication and state.
 * Uses WASM utilities for heavy lifting (encoding, decoding, crypto).
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
	private signers = new Map<string, string>(); // name -> secret key hex

	private signCB = (event: any) => {};
	private eventTarget = new EventTarget();

	public PERPETUAL_SUBSCRIPTIONS = ['notifications', 'starterpack'];

	// In constructor, do NOT start init directly. Just set up ready lazily.
	constructor() {
		const wsRequest = initializeRingHeader(1 * 1024 * 1024); // 1MB (ws request)
		const wsResponse = initializeRingHeader(5 * 1024 * 1024); // 2MB (ws response)

		const wsSignerRequest = initializeRingHeader(512 * 1024);
		const wsSignerResponse = initializeRingHeader(512 * 1024);

		const cacheRequest = initializeRingHeader(1 * 1024 * 1024);
		const cacheResponse = initializeRingHeader(10 * 1024 * 1024);

		const dbRing = initializeRingHeader(2 * 1024 * 1024);

		const signerRequest = initializeRingHeader(512 * 1024);
		const signerResponse = initializeRingHeader(512 * 1024);

		// console.log('connection.url', import.meta.url);

		const connectionURL = new URL('./connections/index.js', import.meta.url);
		const cacheURL = new URL('./cache/index.js', import.meta.url);
		const parserURL = new URL('./parser/index.js', import.meta.url);
		const signerURL = new URL('./signer/index.js', import.meta.url);

		this.connections = new Worker(connectionURL, {
			type: 'module'
		});

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
	}

	// private _ready: Promise<void> | null = null;
	// private config!: NostrManagerConfig;

	// Helper so all calls route through a single place and never race init:
	private postToWorker(message: any, transfer?: Transferable[]) {
		// return this.init().then(() => {
		if (transfer && transfer.length) {
			this.parser.postMessage(message, transfer);
		} else {
			this.parser.postMessage(message);
		}
		// });
	}

	private setupWorkerListener() {
		this.parser.onmessage = async (event) => {
			const id = typeof event.data === 'string' ? event.data : undefined;

			if (!id) return;
			// Prefer O(1) routing via your existing maps
			if (this.subscriptions.has(id)) {
				// Notify only the listeners for this subscription
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
							if (!nostr.nip04) throw new Error('NIP-04 not supported by extension');
							result = await nostr.nip04.encrypt(payload.pubkey, payload.plaintext);
							break;
						case 'nip04Decrypt':
							if (!nostr.nip04) throw new Error('NIP-04 not supported by extension');
							result = await nostr.nip04.decrypt(payload.pubkey, payload.ciphertext);
							break;
						case 'nip44Encrypt':
							if (!nostr.nip44) throw new Error('NIP-44 not supported by extension');
							result = await nostr.nip44.encrypt(payload.pubkey, payload.plaintext);
							break;
						case 'nip44Decrypt':
							if (!nostr.nip44) throw new Error('NIP-44 not supported by extension');
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

			try {
				if (msg.op == 'sign_event') {
					const parsed = JSON.parse(msg.result);
					this.signCB(parsed);
				}
			} catch (error) {
				// console.error("Error parsing event data:", error);
			}
		};

		// this.worker.onerror = (error) => {
		// 	console.error('Worker error:', error);
		// };
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

		let initialMessage: Uint8Array<ArrayBufferLike> = new Uint8Array();

		const buffer = new SharedArrayBuffer(bufferSize + initialMessage.length);

		// Initialize the buffer (sets write position to 4)
		SharedBufferReader.initializeBuffer(buffer);

		// Write the initial message if provided
		if (initialMessage.length > 0) {
			const success = SharedBufferReader.writeMessage(buffer, initialMessage);
			if (!success) {
				console.error('Failed to write initial message to buffer');
			}
		}

		this.subscriptions.set(subId, {
			buffer,
			options,
			refCount: 1
		});

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

		// Wrap in MainMessageT as Subscribe variant
		const mainT = new MainMessageT(MainContent.Subscribe, subscribeT);

		// Serialize with FlatBuffers builder
		const builder = new flatbuffers.Builder(2048);
		const mainOffset = mainT.pack(builder);
		builder.finish(mainOffset);
		const serializedMessage = builder.asUint8Array();

		try {
			// nipWorker.resetInputLoopBackoff();
			void this.postToWorker({ serializedMessage, sharedBuffer: buffer });

			return buffer;
		} catch (error) {
			this.subscriptions.delete(subId);
			throw error;
		}
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
			// this.connections.postMessage(subId);
		}
	}

	publish(publish_id: string, event: NostrEvent, defaultRelays: string[] = []): SharedArrayBuffer {
		// a publish buffer fit in 3kb
		const buffer = new SharedArrayBuffer(3072);

		// Initialize the buffer (sets write position to 4)
		SharedBufferReader.initializeBuffer(buffer);

		try {
			const templateT = new TemplateT(
				event.kind,
				event.created_at,
				this.textEncoder.encode(event.content),
				event.tags.map((t) => new StringVecT(t)) || []
			);
			const publishT = new PublishT(this.textEncoder.encode(publish_id), templateT, defaultRelays);

			// Wrap in MainMessageT as Publish variant
			const mainT = new MainMessageT(MainContent.Publish, publishT);

			// Serialize with FlatBuffers builder
			const builder = new flatbuffers.Builder(2048);
			const mainOffset = mainT.pack(builder);
			builder.finish(mainOffset);
			const serializedMessage = builder.asUint8Array();

			// nipWorker.resetInputLoopBackoff();
			this.postToWorker({ serializedMessage, sharedBuffer: buffer });

			this.publishes.set(publish_id, { buffer });
			return buffer;
		} catch (error) {
			console.error('Failed to publish event:', error);
			throw error;
		}
	}

	setSigner(name: string, payload: string | { url: string; clientSecret: string }): void {
		console.log('Setting signer:', name, payload);
		switch (name) {
			case 'privkey':
				// Create the PrivateKeyT object
				this.signer.postMessage({ type: 'set_private_key', payload });
				break;
			case 'nip07':
				this.signer.postMessage({ type: 'set_nip07' });
				break;
			case 'nip46_bunker':
				// Pass the bunker URL directly
				this.signer.postMessage({ type: 'set_nip46_bunker', payload });
				break;
			case 'nip46_qr':
				// Pass the nostrconnect URL directly
				this.signer.postMessage({ type: 'set_nip46_qr', payload });
				break;
		}

		this.signers.set(name, typeof payload === 'string' ? payload : payload.url);
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

		// Serialize with FlatBuffers builder
		const builder = new flatbuffers.Builder(2048);
		const mainOffset = mainT.pack(builder);
		builder.finish(mainOffset);
		const serializedMessage = builder.asUint8Array();

		this.parser.postMessage(serializedMessage);
	}

	/**
	 * Set up NIP-46 remote signer using a bunker URL.
	 * This is for the "Direct connection initiated by remote-signer" flow.
	 * @param bunkerUrl The bunker URL in format: bunker://<remote-signer-pubkey>?relay=<relay-url>&relay=<relay-url>&secret=<secret>
	 */
	setNip46Bunker(bunkerUrl: string): void {
		console.log('Setting up NIP-46 with bunker URL:', bunkerUrl);
		this.setSigner('nip46_bunker', bunkerUrl);
	}

	/**
	 * Set up NIP-46 remote signer using a QR code.
	 * This is for the "Direct connection initiated by the client" flow.
	 * @param nostrconnectUrl The nostrconnect URL from the QR code
	 * @param clientSecret Optional client secret key (hex)
	 */
	setNip46QR(nostrconnectUrl: string, clientSecret?: string): void {
		console.log('Setting up NIP-46 with QR code:', nostrconnectUrl);
		this.setSigner(
			'nip46_qr',
			clientSecret ? { url: nostrconnectUrl, clientSecret } : nostrconnectUrl
		);
	}

	/**
	 * Set up NIP-07 extension signer (window.nostr).
	 */
	setNip07(): void {
		console.log('Setting up NIP-07 extension signer');
		this.setSigner('nip07', '');
	}

	cleanup(): void {
		// console.trace('Cleanup called');
		const subscriptionsToDelete: string[] = [];
		for (const [subId, subscription] of this.subscriptions.entries()) {
			if (subscription.refCount <= 0 && !this.PERPETUAL_SUBSCRIPTIONS.includes(subId)) {
				subscriptionsToDelete.push(subId);
			}
		}
		// for (const subId of subscriptionsToDelete) {
		// 	const subscription = this.subscriptions.get(subId);
		// 	if (subscription) {
		// 		const mainT = new MainMessageT(
		// 			MainContent.Unsubscribe,
		// 			new UnsubscribeT(this.textEncoder.encode(subId))
		// 		);
		// 		// Serialize with FlatBuffers builder
		// 		const builder = new flatbuffers.Builder(2048);
		// 		const mainOffset = mainT.pack(builder);
		// 		builder.finish(mainOffset);
		// 		const serializedMessage = builder.asUint8Array();
		// 		// nipWorker.resetInputLoopBackoff();
		// 		this.postToWorker(serializedMessage);
		// 		subscription.closed = true;
		// 		this.subscriptions.delete(subId);
		// 	}
		// }
	}
}

export const manager = new NostrManager();

export * from './types';
