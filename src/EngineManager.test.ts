import { describe, it, expect, vi, beforeAll, afterAll } from 'vitest';
import { EngineManager } from './EngineManager';
import * as flatbuffers from 'flatbuffers';
import {
	MainContent,
	MainMessage,
	Message,
	MessageType,
	Nip46BunkerT,
	Nip46QRT,
	Raw,
	SetSigner,
	SignerType,
	WorkerMessage
} from './generated/nostr/fb';

// Mock the engine worker so we can send messages back to EngineManager
class MockWorker {
	public onmessage: ((event: MessageEvent) => void) | null = null;
	public port2: MockMessagePort;
	public port1: MockMessagePort;

	constructor() {
		const channel = new MessageChannel();
		this.port1 = channel.port1 as any;
		this.port2 = channel.port2 as any;
	}

	postMessage(msg: any, transfer?: any[]) {
		// The EngineManager sends { serializedMessage } to init the worker,
		// and later sends messages via port2. We don't need to simulate the worker internals.
	}
}

beforeAll(() => {
	(globalThis as any).Worker = MockWorker;
	(globalThis as any).localStorage = {
		getItem: vi.fn(),
		setItem: vi.fn(),
		removeItem: vi.fn()
	};
});

afterAll(() => {
	delete (globalThis as any).Worker;
});

function buildCryptoRawMessage(json: string): Uint8Array {
	const builder = new flatbuffers.Builder(256);
	const rawStr = builder.createString(json);
	const raw = Raw.createRaw(builder, rawStr);
	const msg = WorkerMessage.createWorkerMessage(builder, 0, 0, MessageType.Raw, Message.Raw, raw);
	builder.finish(msg);
	return builder.asUint8Array();
}

describe('EngineManager', () => {
	describe('handleCryptoMessage', () => {
		it('should dispatch auth event on set_signer success', () => {
			const manager = new EngineManager();
			const authHandler = vi.fn();
			manager.addEventListener('auth', authHandler);

			// Simulate a pending privkey session
			(manager as any)._pendingSession = { type: 'privkey', payload: 'secret' };

			// Build the crypto message with 4-byte length prefix (as the WASM engine sends)
			const workerBytes = buildCryptoRawMessage(
				JSON.stringify({
					op: 'set_signer',
					result: '79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798'
				})
			);
			const data = new Uint8Array(4 + workerBytes.length);
			const view = new DataView(data.buffer);
			view.setUint32(0, workerBytes.length, true);
			data.set(workerBytes, 4);

			(manager as any).handleCryptoMessage(data.buffer);

			expect(authHandler).toHaveBeenCalledTimes(1);
			const detail = authHandler.mock.calls[0][0].detail;
			expect(detail.pubkey).toBe(
				'79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798'
			);
			expect(detail.hasSigner).toBe(true);
			expect(detail.secretKey).toBe('secret');
			expect((manager as any)._pendingSession).toBeNull();
		});

		it('should request public key when NIP-46 set_signer only confirms initialization', () => {
			const manager = new EngineManager();
			const authHandler = vi.fn();
			const getPublicKey = vi.spyOn(manager, 'getPublicKey').mockImplementation(() => {});
			manager.addEventListener('auth', authHandler);
			(manager as any)._pendingSession = {
				type: 'nip46',
				payload: { url: 'bunker://remote?relay=wss%3A%2F%2Fr', clientSecret: 'secret' }
			};

			const workerBytes = buildCryptoRawMessage(
				JSON.stringify({
					op: 'set_signer',
					result: 'NIP-46 QR signer initialized, awaiting discovery'
				})
			);
			const data = new Uint8Array(4 + workerBytes.length);
			const view = new DataView(data.buffer);
			view.setUint32(0, workerBytes.length, true);
			data.set(workerBytes, 4);

			(manager as any).handleCryptoMessage(data.buffer);

			expect(authHandler).not.toHaveBeenCalled();
			expect(getPublicKey).toHaveBeenCalledTimes(1);
			expect((manager as any)._pendingSession?.type).toBe('nip46');
		});

		it('should dispatch auth event on get_public_key success', () => {
			const manager = new EngineManager();
			const authHandler = vi.fn();
			manager.addEventListener('auth', authHandler);

			const workerBytes = buildCryptoRawMessage(
				JSON.stringify({
					op: 'get_public_key',
					result: 'abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890'
				})
			);
			const data = new Uint8Array(4 + workerBytes.length);
			const view = new DataView(data.buffer);
			view.setUint32(0, workerBytes.length, true);
			data.set(workerBytes, 4);

			(manager as any).handleCryptoMessage(data.buffer);

			expect(authHandler).toHaveBeenCalledTimes(1);
			const detail = authHandler.mock.calls[0][0].detail;
			expect(detail.pubkey).toBe(
				'abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890'
			);
			expect(detail.hasSigner).toBe(true);
		});

		it('should call _signCB on sign_event success', () => {
			const manager = new EngineManager();
			const signHandler = vi.fn();
			(manager as any)._signCB = signHandler;

			const signedEvent = {
				id: 'event-id',
				pubkey: 'pubkey',
				created_at: 123,
				kind: 1,
				tags: [],
				content: 'hello',
				sig: 'sig'
			};
			const workerBytes = buildCryptoRawMessage(
				JSON.stringify({ op: 'sign_event', result: JSON.stringify(signedEvent) })
			);
			const data = new Uint8Array(4 + workerBytes.length);
			const view = new DataView(data.buffer);
			view.setUint32(0, workerBytes.length, true);
			data.set(workerBytes, 4);

			(manager as any).handleCryptoMessage(data.buffer);

			expect(signHandler).toHaveBeenCalledTimes(1);
			expect(signHandler).toHaveBeenCalledWith(signedEvent);
		});

		it('should dispatch auth with hasSigner=false on error response', () => {
			const manager = new EngineManager();
			const authHandler = vi.fn();
			manager.addEventListener('auth', authHandler);

			const workerBytes = buildCryptoRawMessage(
				JSON.stringify({ op: 'get_public_key', error: 'signer not available' })
			);
			const data = new Uint8Array(4 + workerBytes.length);
			const view = new DataView(data.buffer);
			view.setUint32(0, workerBytes.length, true);
			data.set(workerBytes, 4);

			(manager as any).handleCryptoMessage(data.buffer);

			expect(authHandler).toHaveBeenCalledTimes(1);
			const detail = authHandler.mock.calls[0][0].detail;
			expect(detail.pubkey).toBeNull();
			expect(detail.hasSigner).toBe(false);
		});
	});

	describe('setSigner', () => {
		it('packs NIP-46 bunker signer fields in schema order', () => {
			const manager = new EngineManager();
			let sent: Uint8Array | undefined;
			(manager as any).postMessage = (message: any) => {
				sent = message.serializedMessage;
			};

			manager.setSigner('nip46', {
				url: 'bunker://remote?relay=wss%3A%2F%2Fr',
				clientSecret: 'secret'
			});

			const main = MainMessage.getRootAsMainMessage(new flatbuffers.ByteBuffer(sent!));
			expect(main.contentType()).toBe(MainContent.SetSigner);
			const setSigner = main.content(new SetSigner())!;
			expect(setSigner.signerTypeType()).toBe(SignerType.Nip46Bunker);
			const unpacked = setSigner.unpack().signerType as Nip46BunkerT;
			expect(unpacked.bunkerUrl).toBe('bunker://remote?relay=wss%3A%2F%2Fr');
			expect(unpacked.clientSecret).toBe('secret');
		});

		it('packs NIP-46 QR signer fields in schema order', () => {
			const manager = new EngineManager();
			let sent: Uint8Array | undefined;
			(manager as any).postMessage = (message: any) => {
				sent = message.serializedMessage;
			};

			manager.setSigner('nip46', {
				url: 'nostrconnect://client?relay=wss%3A%2F%2Fr&secret=s',
				clientSecret: 'secret'
			});

			const main = MainMessage.getRootAsMainMessage(new flatbuffers.ByteBuffer(sent!));
			expect(main.contentType()).toBe(MainContent.SetSigner);
			const setSigner = main.content(new SetSigner())!;
			expect(setSigner.signerTypeType()).toBe(SignerType.Nip46QR);
			const unpacked = setSigner.unpack().signerType as Nip46QRT;
			expect(unpacked.nostrconnectUrl).toBe('nostrconnect://client?relay=wss%3A%2F%2Fr&secret=s');
			expect(unpacked.clientSecret).toBe('secret');
		});
	});
});
