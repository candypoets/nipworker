import * as flatbuffers from 'flatbuffers';
import { ArrayBufferReader } from 'src/lib/ArrayBufferReader';
import { BaseBackend, localStorageAdapter } from 'src/lib/BaseBackend';
import { NostrManagerConfig, RequestObject, SubscriptionConfig } from 'src/types';
import type { EventTemplate, NostrEvent } from 'nostr-tools';
import {
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
	Pubkey,
	Raw,
	RequestT,
	SetSignerT,
	SignEventT,
	SignedEvent,
	SignerType,
	StringVec,
	StringVecT,
	SubscribeT,
	SubscriptionConfigT,
	TemplateT,
	UnsubscribeT,
	WorkerMessage
} from './generated/nostr/fb';

/**
 * EngineManager is a single-worker backend for nipworker-core.
 * It spawns one WASM worker (nipworker-engine) that hosts the full
 * NostrEngine (transport + storage + parser + crypto) internally.
 */
export class EngineManager extends BaseBackend {
	private worker: Worker;
	private enginePort: MessagePort;
	private _signCB = (_event: NostrEvent) => {};

	constructor(_config: NostrManagerConfig = {}) {
		super(localStorageAdapter);
		const engineURL = new URL('./engine/index.ts', import.meta.url);
		this.worker = new Worker(engineURL, { type: 'module' });

		const mainPort = new MessageChannel();
		this.enginePort = mainPort.port1;

		this.worker.postMessage(
			{ type: 'init', payload: { port: mainPort.port2, logLevel: _config.logLevel } },
			[mainPort.port2]
		);

		this.enginePort.onmessage = async (event) => {
			const { subId, data, type, status, url } = event.data;

			if (subId && data) {
				if (subId === 'crypto') {
					this.handleCryptoMessage(data);
					return;
				}
				const subscription = this.subscriptions.get(subId);
				if (subscription) {
					const written = ArrayBufferReader.writeBatchedData(subscription.buffer, data, subId);
					if (written) {
						this.dispatch(`subscription:${subId}`, subId);
					}
					return;
				}
				const publish = this.publishes.get(subId);
				if (publish) {
					const written = ArrayBufferReader.writeBatchedData(publish.buffer, data, subId);
					if (written) {
						this.dispatch(`publish:${subId}`, subId);
					}
					return;
				}
			}

			// Handle direct engine responses (crypto / sign-event) with empty subId
			if (data && (subId === '' || subId === null || subId === undefined)) {
				const uint8Data = new Uint8Array(data);
				if (uint8Data.length >= 4) {
					const view = new DataView(uint8Data.buffer, uint8Data.byteOffset, uint8Data.byteLength);
					const payloadLen = view.getUint32(0, true);
					if (uint8Data.length >= 4 + payloadLen) {
						const bb = new flatbuffers.ByteBuffer(uint8Data.subarray(4, 4 + payloadLen));
						const workerMsg = WorkerMessage.getRootAsWorkerMessage(bb);
						const msgType = workerMsg.type();
						if (msgType === MessageType.Pubkey) {
							const pubkeyObj = workerMsg.content(new Pubkey());
							const pubkey = pubkeyObj ? pubkeyObj.pubkey() : null;
							if (pubkey) {
								this.handleCryptoResponse({ type: 'response', op: 'get_pubkey', ok: true, result: pubkey });
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
				}
				return;
			}

			if (type === 'extension_request') {
				const { id, op, payload } = event.data;
				try {
					const nostr = (window as any).nostr;
					if (!nostr) throw new Error('NIP-07 extension (window.nostr) not found');

					let result;
					switch (op) {
						case 'getPublicKey':
							result = await nostr.getPublicKey();
							break;
						case 'signEvent':
							result = await nostr.signEvent(payload);
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
					this.postMessage({ type: 'extension_response', id, ok: true, result });
				} catch (e: any) {
					this.postMessage({ type: 'extension_response', id, ok: false, error: e.message || String(e) });
				}
				return;
			}

			if (type === 'relay:status' && url && status) {
				this.relayStatuses.set(url, { status, timestamp: Date.now() });
				this.dispatch('relay:status', { status, url });
			}

			if (type === 'response' && event.data.op === 'get_pubkey') {
				this.handleCryptoResponse(event.data);
			}
			if (type === 'response' && event.data.op === 'sign_event' && event.data.ok) {
				try {
					const parsed = JSON.parse(event.data.result);
					this._signCB(parsed);
				} catch (e) {
					console.warn('[EngineManager] Failed to parse signed event', e);
				}
			}
		};

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
				this.worker.postMessage({ type: 'wake', source: 'visibility' });
			}
		});
	}

	protected onLogout(): void {
		this.postMessage({ type: 'clear_signer' });
	}

	private postMessage(message: any, transfer?: Transferable[]) {
		this.enginePort.postMessage(message, transfer || []);
	}

	private handleCryptoResponse(msg: any) {
		if (msg.op === 'get_pubkey') {
			if (msg.ok) {
				this.activePubkey = msg.result;
				if (this._pendingSession) {
					this.saveSession(this.activePubkey!, this._pendingSession.type, this._pendingSession.payload);
					this._pendingSession = null;
				}
			}
			const secretKey =
				this._pendingSession?.type === 'privkey' ? this._pendingSession.payload : undefined;
			this.dispatch('auth', { pubkey: this.activePubkey, hasSigner: msg.ok, secretKey });
		}
	}

	private handleCryptoMessage(data: ArrayBuffer): void {
		const uint8Data = new Uint8Array(data);
		if (uint8Data.length < 4) return;
		const view = new DataView(uint8Data.buffer, uint8Data.byteOffset, uint8Data.byteLength);
		const payloadLen = view.getUint32(0, true);
		if (uint8Data.length < 4 + payloadLen) return;
		const bb = new flatbuffers.ByteBuffer(uint8Data.subarray(4, 4 + payloadLen));
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
				console.warn('[EngineManager] Crypto error:', msg.error);
			}
		} catch (e) {
			console.warn('[EngineManager] Failed to parse crypto raw message:', e);
		}
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
		this.postMessage({ serializedMessage: uint8Array }, [uint8Array.buffer]);

		return buffer;
	}

	publish(publish_id: string, event: NostrEvent, defaultRelays: string[] = [], optimisticSubIds?: string[]): ArrayBuffer {
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
		this.postMessage({ serializedMessage: uint8Array }, [uint8Array.buffer]);
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
				const pkT = new PrivateKeyT(payload as string);
				const setSignerT = new SetSignerT(SignerType.PrivateKey, pkT);
				const mainT = new MainMessageT(MainContent.SetSigner, setSignerT);
				const builder = new flatbuffers.Builder(2048);
				builder.finish(mainT.pack(builder));
				const uint8Array = builder.asUint8Array();
				this.postMessage({ serializedMessage: uint8Array }, [uint8Array.buffer]);
				break;
			}
			case 'nip07': {
				const nip07T = new Nip07T();
				const setSignerT = new SetSignerT(SignerType.Nip07, nip07T);
				const mainT = new MainMessageT(MainContent.SetSigner, setSignerT);
				const builder = new flatbuffers.Builder(2048);
				builder.finish(mainT.pack(builder));
				const uint8Array = builder.asUint8Array();
				this.postMessage({ serializedMessage: uint8Array }, [uint8Array.buffer]);
				break;
			}
			case 'nip46': {
				const nip46Payload = payload as { url: string; clientSecret: string } | undefined;
				if (nip46Payload?.url) {
					if (nip46Payload.url.startsWith('bunker://')) {
						const bunkerT = new Nip46BunkerT(nip46Payload.clientSecret, nip46Payload.url);
						const setSignerT = new SetSignerT(SignerType.Nip46Bunker, bunkerT);
						const mainT = new MainMessageT(MainContent.SetSigner, setSignerT);
						const builder = new flatbuffers.Builder(2048);
						builder.finish(mainT.pack(builder));
						const uint8Array = builder.asUint8Array();
						this.postMessage({ serializedMessage: uint8Array }, [uint8Array.buffer]);
					} else if (nip46Payload.url.startsWith('nostrconnect://')) {
						const qrT = new Nip46QRT(nip46Payload.clientSecret, nip46Payload.url);
						const setSignerT = new SetSignerT(SignerType.Nip46QR, qrT);
						const mainT = new MainMessageT(MainContent.SetSigner, setSignerT);
						const builder = new flatbuffers.Builder(2048);
						builder.finish(mainT.pack(builder));
						const uint8Array = builder.asUint8Array();
						this.postMessage({ serializedMessage: uint8Array }, [uint8Array.buffer]);
					}
				}
				this.getPublicKey();
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
		this.postMessage({ serializedMessage: uint8Array }, [uint8Array.buffer]);
	}

	getPublicKey() {
		const mainT = new MainMessageT(MainContent.GetPublicKey, new GetPublicKeyT());
		const builder = new flatbuffers.Builder(2048);
		builder.finish(mainT.pack(builder));
		const uint8Array = builder.asUint8Array();
		this.postMessage({ serializedMessage: uint8Array }, [uint8Array.buffer]);
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
			this.postMessage({ serializedMessage: uint8Array }, [uint8Array.buffer]);
			this.subscriptions.delete(subId);
		}
	}
}
