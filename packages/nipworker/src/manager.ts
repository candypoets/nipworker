import RustWorker from '@candypoets/rust-worker/worker.js?worker';
import wasmAsset from '@candypoets/rust-worker/rust_worker_bg.wasm?url';
import type { EventTemplate, NostrEvent } from 'nostr-tools';

import { SharedBufferReader } from 'src/lib/SharedBuffer';

import * as flatbuffers from 'flatbuffers';
import { RequestObject, SubscriptionConfig } from 'src/types';
import {
	GetPublicKeyT,
	MainContent,
	MainMessageT,
	PipelineConfigT,
	PrivateKeyT,
	PublishT,
	RequestT,
	SetSignerT,
	SignerType,
	SignEventT,
	StringVecT,
	SubscribeT,
	SubscriptionConfigT,
	TemplateT,
	UnsubscribeT
} from './generated/nostr/fb';

/**
 * Configuration for the Nostr Manager
 */
export interface NostrManagerConfig {
	bufferKey: string;
	maxBufferSize: number;
	inRing: SharedArrayBuffer;
	outRing: SharedArrayBuffer;
}

// Globals for single fetch + per-worker copies
let originalWasmBuffer: ArrayBuffer | null = null;
let fetchPromise: Promise<void> | null = null;

async function ensureOriginalBuffer(): Promise<void> {
	if (fetchPromise) {
		return fetchPromise;
	}

	fetchPromise = fetch(wasmAsset)
		.then((res) => {
			if (!res.ok) {
				throw new Error(`Failed to fetch WASM: ${res.status} ${res.statusText}`);
			}
			return res.arrayBuffer();
		})
		.then((buffer) => {
			originalWasmBuffer = buffer; // Cache the original (never transfer this one)
		})
		.catch((error) => {
			console.error('WASM fetch failed:', error);
			throw error;
		});

	return fetchPromise;
}

/**
 * Pure TypeScript NostrClient that manages worker communication and state.
 * Uses WASM utilities for heavy lifting (encoding, decoding, crypto).
 */
export class NostrManager {
	private worker: Worker;
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
	constructor(config: NostrManagerConfig) {
		this.config = config;
		// Optionally prewarm, but at idle priority:
		const prewarm = () => void this.init();
		if ('requestIdleCallback' in window) {
			(window as any).requestIdleCallback(prewarm, { timeout: 2000 });
		} else {
			setTimeout(prewarm, 1500);
		}
	}

	private _ready: Promise<void> | null = null;
	private config!: NostrManagerConfig;

	private init(): Promise<void> {
		if (this._ready) return this._ready;

		this.worker = this.createWorker();
		this.setupWorkerListener();

		this._ready = (async () => {
			await ensureOriginalBuffer();
			// Create a transferable copy for the worker to avoid an extra clone.
			const wasmCopy = originalWasmBuffer!.slice(0);
			this.worker.postMessage(
				{
					type: 'init',
					payload: {
						bufferKey: this.config.bufferKey,
						maxBufferSize: this.config.maxBufferSize,
						wasmBuffer: wasmCopy,
						inRing: this.config.inRing,
						outRing: this.config.outRing
					}
				},
				// Transfer the wasm copy to the worker (no duplicate copy).
				[wasmCopy]
			);
		})();

		return this._ready;
	}

	// Helper so all calls route through a single place and never race init:
	private postToWorker(message: any, transfer?: Transferable[]) {
		return this.init().then(() => {
			if (transfer && transfer.length) {
				this.worker.postMessage(message, transfer);
			} else {
				this.worker.postMessage(message);
			}
		});
	}

	private createWorker(): Worker {
		return new RustWorker();
	}

	private setupWorkerListener() {
		this.worker.onmessage = async (event) => {
			const id = typeof event.data === 'string' ? event.data : undefined;
			try {
				if (event.data.startsWith('{"id":')) {
					const parsed = JSON.parse(event.data);
					this.signCB(parsed);
				}
			} catch (error) {
				// console.error("Error parsing event data:", error);
			}

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

		this.worker.onerror = (error) => {
			console.error('Worker error:', error);
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
			options.bytesPerEvent
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
		}
	}

	publish(publish_id: string, event: NostrEvent): SharedArrayBuffer {
		// a publish buffer fit in 3kb
		const buffer = new SharedArrayBuffer(3072);

		// Initialize the buffer (sets write position to 4)
		SharedBufferReader.initializeBuffer(buffer);

		try {
			const templateT = new TemplateT(
				event.kind,
				this.textEncoder.encode(event.content),
				event.tags.map((t) => new StringVecT(t)) || []
			);
			const publishT = new PublishT(this.textEncoder.encode(publish_id), templateT);

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

	setSigner(name: string, secretKeyHex: string): void {
		console.log('setSigner', name, secretKeyHex);

		// Create the PrivateKeyT object
		const privateKeyT = new PrivateKeyT(this.textEncoder.encode(secretKeyHex));

		// Create the SetSignerT object and set the union
		const setSignerT = new SetSignerT(SignerType.PrivateKey, privateKeyT);

		// Create the MainMessageT with the properly constructed SetSignerT
		const mainT = new MainMessageT(MainContent.SetSigner, setSignerT);

		// Serialize with FlatBuffers builder (unchanged)
		const builder = new flatbuffers.Builder(2048);
		const mainOffset = mainT.pack(builder);
		builder.finish(mainOffset);
		const serializedMessage = builder.asUint8Array();

		void this.postToWorker(serializedMessage);
		this.signers.set(name, secretKeyHex);
	}

	signEvent(event: EventTemplate, cb: (event: NostrEvent) => void) {
		const mainT = new MainMessageT(
			MainContent.SignEvent,
			new SignEventT(
				new TemplateT(
					event.kind,
					this.textEncoder.encode(event.content),
					event.tags.map((t) => new StringVecT(t))
				)
			)
		);

		// Serialize with FlatBuffers builder
		const builder = new flatbuffers.Builder(2048);
		const mainOffset = mainT.pack(builder);
		builder.finish(mainOffset);
		const serializedMessage = builder.asUint8Array();
		this.signCB = cb;
		this.worker.postMessage(serializedMessage);
	}

	getPublicKey() {
		const mainT = new MainMessageT(MainContent.GetPublicKey, new GetPublicKeyT());

		// Serialize with FlatBuffers builder
		const builder = new flatbuffers.Builder(2048);
		const mainOffset = mainT.pack(builder);
		builder.finish(mainOffset);
		const serializedMessage = builder.asUint8Array();

		this.worker.postMessage(serializedMessage);
	}

	cleanup(): void {
		const subscriptionsToDelete: string[] = [];

		for (const [subId, subscription] of this.subscriptions.entries()) {
			if (subscription.refCount <= 0 && !this.PERPETUAL_SUBSCRIPTIONS.includes(subId)) {
				subscriptionsToDelete.push(subId);
			}
		}

		for (const subId of subscriptionsToDelete) {
			const subscription = this.subscriptions.get(subId);
			if (subscription) {
				const mainT = new MainMessageT(
					MainContent.Unsubscribe,
					new UnsubscribeT(this.textEncoder.encode(subId))
				);
				// Serialize with FlatBuffers builder
				const builder = new flatbuffers.Builder(2048);
				const mainOffset = mainT.pack(builder);
				builder.finish(mainOffset);
				const serializedMessage = builder.asUint8Array();
				// nipWorker.resetInputLoopBackoff();
				this.postToWorker(serializedMessage);
				subscription.closed = true;
				this.subscriptions.delete(subId);
			}
		}
	}
}

export * from './types';
