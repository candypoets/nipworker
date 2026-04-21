import * as flatbuffers from 'flatbuffers';
import { ArrayBufferReader } from 'src/lib/ArrayBufferReader';
import { NostrManagerConfig, RequestObject, SubscriptionConfig } from 'src/types';
import type { EventTemplate, NostrEvent } from 'nostr-tools';
import { BunkerSigner, parseBunkerInput, toBunkerURL } from 'nostr-tools/nip46';
import { hexToBytes } from 'nostr-tools/utils';
import {
	GetPublicKeyT,
	MainContent,
	MainMessageT,
	MessageType,
	PipelineConfigT,
	PrivateKeyT,
	PublishT,
	Pubkey,
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
export class EngineManager {
	private worker: Worker;
	private enginePort: MessagePort;
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
	private eventTarget = new EventTarget();
	private activePubkey: string | null = null;
	private _pendingSession: { type: string; payload: any } | null = null;
	private _signCB = (_event: NostrEvent) => {};

	private relayStatuses = new Map<
		string,
		{ status: 'connected' | 'failed' | 'close'; timestamp: number }
	>();

	private proxyPending = new Map<
		number,
		{ resolve: (value: string) => void; reject: (err: Error) => void }
	>();
	private proxyNextId = 0;
	private nip46Signer: BunkerSigner | null = null;
	private nip46SignerGeneration = 0;
	private _signerGeneration = 0;

	public PERPETUAL_SUBSCRIPTIONS = ['notifications', 'starterpack'];

	constructor(_config: NostrManagerConfig = {}) {
		const engineURL = new URL('./engine/index.js', import.meta.url);
		this.worker = new Worker(engineURL, { type: 'module' });

		const mainPort = new MessageChannel();
		this.enginePort = mainPort.port1;

		this.worker.postMessage(
			{ type: 'init', payload: { port: mainPort.port2 } },
			[mainPort.port2]
		);

		this.enginePort.onmessage = (event) => {
			const { subId, data, type, status, url } = event.data;

			if (subId && data) {
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

			if (type === 'signer_request') {
				this.handleSignerRequest(event.data);
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

	getBuffer(subId: string): ArrayBuffer | undefined {
		const existing = this.subscriptions.get(subId);
		if (existing) {
			existing.refCount++;
			return existing.buffer;
		}
		return undefined;
	}

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
		this._signerGeneration++;
		const currentGeneration = this._signerGeneration;
		this._pendingSession = { type: name, payload };
		if (name !== 'nip46') {
			this.nip46Signer = null;
			this.nip46SignerGeneration = -1;
		}
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
			case 'nip07':
				this.postMessage({ type: 'set_proxy_signer', signerType: 'nip07' });
				this.getPublicKey();
				break;
			case 'nip46': {
				this.postMessage({ type: 'set_proxy_signer', signerType: 'nip46' });
				this.nip46Signer = null;
				this.nip46SignerGeneration = -1;
				const nip46Payload = payload as { url: string; clientSecret: string } | undefined;
				if (nip46Payload?.url) {
					(async () => {
						try {
							if (nip46Payload.url.startsWith('bunker://')) {
								const bp = await parseBunkerInput(nip46Payload.url);
								if (!bp) throw new Error('Failed to parse bunker URL');
								const secretKey = hexToBytes(nip46Payload.clientSecret);
								const signer = BunkerSigner.fromBunker(secretKey, bp);
								this.nip46Signer = signer;
								this.nip46SignerGeneration = currentGeneration;
								await signer.connect();
								if (this._signerGeneration !== currentGeneration) return;
								this.getPublicKey();
							} else if (nip46Payload.url.startsWith('nostrconnect://')) {
								const signer = await this.initNip46URISigner(nip46Payload.url, nip46Payload.clientSecret);
								if (this._signerGeneration !== currentGeneration) return;
								this.nip46Signer = signer;
								this.nip46SignerGeneration = currentGeneration;
								const bunkerUrl = toBunkerURL(signer.bp);
								this._pendingSession!.payload = { url: bunkerUrl, clientSecret: nip46Payload.clientSecret };
								this.getPublicKey();
							}
						} catch (err: any) {
							console.error('[EngineManager] Failed to initialize NIP-46 signer:', err.message || String(err));
							if (this._signerGeneration !== currentGeneration) return;
							this.nip46Signer = null;
							this.nip46SignerGeneration = -1;
							this.dispatch('auth', { pubkey: null, hasSigner: false });
						}
					})();
				} else {
					this.getPublicKey();
				}
				break;
			}
		}
	}

	private generateClientSecret(): string {
		const array = new Uint8Array(32);
		crypto.getRandomValues(array);
		return Array.from(array, (b) => b.toString(16).padStart(2, '0')).join('');
	}

	setNip46Bunker(bunkerUrl: string, clientSecret?: string): void {
		const secret = clientSecret || this.generateClientSecret();
		console.log('[EngineManager] NIP-46 bunker:', bunkerUrl.slice(0, 50) + '...');
		this.setSigner('nip46', { url: bunkerUrl, clientSecret: secret });
	}

	setNip46QR(nostrconnectUrl: string, clientSecret?: string): void {
		const secret = clientSecret || this.generateClientSecret();
		console.log('[EngineManager] NIP-46 QR:', nostrconnectUrl.slice(0, 50) + '...');
		this.setSigner('nip46', { url: nostrconnectUrl, clientSecret: secret });
	}

	setNip07(): void {
		this.setSigner('nip07', '');
	}

	private async initNip46URISigner(nostrconnectUrl: string, clientSecret: string): Promise<BunkerSigner> {
		const secretKey = hexToBytes(clientSecret);
		const signer = await BunkerSigner.fromURI(secretKey, nostrconnectUrl);
		return signer;
	}

	private getActiveSessionType(): string | null {
		if (this._pendingSession) return this._pendingSession.type;
		const pubkey = this.activePubkey || localStorage.getItem('nostr_active_pubkey');
		if (!pubkey) return null;
		const accounts = this.getAccounts();
		return accounts[pubkey]?.type || null;
	}

	private async handleSignerRequest(msg: any): Promise<void> {
		const { id, op, payload } = msg;
		// Maintain pending map structure as required by M12B
		this.proxyNextId++;
		this.proxyPending.set(id, {
			resolve: () => {},
			reject: () => {}
		});
		try {
			const sessionType = this.getActiveSessionType();
			let result: string;

			if (sessionType === 'nip07') {
				result = await this.executeNip07Op(op, payload);
			} else if (sessionType === 'nip46') {
				result = await this.executeNip46Op(op, payload);
			} else {
				throw new Error(`No active proxy signer session (type: ${sessionType})`);
			}

			this.postMessage({ type: 'signer_response', id, result });
		} catch (err: any) {
			this.postMessage({
				type: 'signer_response',
				id,
				error: err.message || String(err)
			});
		} finally {
			this.proxyPending.delete(id);
		}
	}

	private async executeNip07Op(op: string, payload: any): Promise<string> {
		const nostr = (window as any).nostr;
		if (!nostr) throw new Error('NIP-07 extension (window.nostr) not found');

		switch (op) {
			case 'get_public_key':
				if (typeof nostr.getPublicKey !== 'function') {
					throw new Error('window.nostr.getPublicKey is not available');
				}
				return await nostr.getPublicKey();
			case 'sign_event': {
				if (typeof nostr.signEvent !== 'function') {
					throw new Error('window.nostr.signEvent is not available');
				}
				const ev = JSON.parse(payload.event_json);
				const signed = await nostr.signEvent(ev);
				return JSON.stringify(signed);
			}
			case 'nip04_encrypt':
				if (!nostr.nip04 || typeof nostr.nip04.encrypt !== 'function') {
					throw new Error('window.nostr.nip04.encrypt is not available');
				}
				return await nostr.nip04.encrypt(payload.peer, payload.plaintext);
			case 'nip04_decrypt':
				if (!nostr.nip04 || typeof nostr.nip04.decrypt !== 'function') {
					throw new Error('window.nostr.nip04.decrypt is not available');
				}
				return await nostr.nip04.decrypt(payload.peer, payload.ciphertext);
			case 'nip44_encrypt':
				if (!nostr.nip44 || typeof nostr.nip44.encrypt !== 'function') {
					throw new Error('window.nostr.nip44.encrypt is not available');
				}
				return await nostr.nip44.encrypt(payload.peer, payload.plaintext);
			case 'nip44_decrypt':
				if (!nostr.nip44 || typeof nostr.nip44.decrypt !== 'function') {
					throw new Error('window.nostr.nip44.decrypt is not available');
				}
				return await nostr.nip44.decrypt(payload.peer, payload.ciphertext);
			default:
				throw new Error(`Unknown signer operation: ${op}`);
		}
	}

	private async executeNip46Op(op: string, payload: any): Promise<string> {
		if (!this.nip46Signer || this.nip46SignerGeneration !== this._signerGeneration) {
			throw new Error('NIP-46 signer not initialized');
		}

		switch (op) {
			case 'get_public_key':
				return await this.nip46Signer.getPublicKey();
			case 'sign_event': {
				const ev = JSON.parse(payload.event_json);
				const signed = await this.nip46Signer.signEvent(ev);
				return JSON.stringify(signed);
			}
			case 'nip04_encrypt':
				return await this.nip46Signer.nip04Encrypt(payload.peer, payload.plaintext);
			case 'nip04_decrypt':
				return await this.nip46Signer.nip04Decrypt(payload.peer, payload.ciphertext);
			case 'nip44_encrypt':
				return await this.nip46Signer.nip44Encrypt(payload.peer, payload.plaintext);
			case 'nip44_decrypt':
				return await this.nip46Signer.nip44Decrypt(payload.peer, payload.ciphertext);
			default:
				throw new Error(`Unknown signer operation: ${op}`);
		}
	}

	setPubkey(pubkey: string): void {
		this.setSigner('pubkey', pubkey);
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

	getActivePubkey(): string | null {
		return this.activePubkey;
	}

	getSubscriptionCount(): number {
		return this.subscriptions.size;
	}

	getAccounts(): Record<string, { type: string; payload: any }> {
		const accountsJson = localStorage.getItem('nostr_signer_accounts') || '{}';
		try {
			return JSON.parse(accountsJson);
		} catch (e) {
			return {};
		}
	}

	switchAccount(pubkey: string) {
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
		} else {
			this.dispatch('auth', { pubkey: null, hasSigner: false });
		}
	}

	public logout(): void {
		this._signerGeneration++;
		this.nip46Signer = null;
		this.nip46SignerGeneration = -1;
		this._pendingSession = null;
		this.activePubkey = null;
		this.postMessage({ type: 'clear_signer' });
		localStorage.removeItem('nostr_active_pubkey');
		this.dispatch('logout');
	}

	public removeAccount(): void {
		const currentPubkey = this.activePubkey;
		if (!currentPubkey) return;
		const accounts = this.getAccounts();
		delete accounts[currentPubkey];
		localStorage.setItem('nostr_signer_accounts', JSON.stringify(accounts));
		const remaining = Object.keys(accounts);
		if (remaining.length > 0 && remaining[0]) {
			this.switchAccount(remaining[0]);
		} else {
			this.logout();
		}
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
