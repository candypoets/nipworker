import { describe, it, expect, vi, beforeAll, afterAll } from 'vitest';
import { NativeBackend } from './NativeBackend';
import * as flatbuffers from 'flatbuffers';
import { WorkerMessage, MessageType, Raw, Message } from './generated/nostr/fb';

const mockNativeModule = {
	init: vi.fn(),
	handleMessage: vi.fn(),
	setPrivateKey: vi.fn(),
	deinit: vi.fn(),
};

const mockEmitter = {
	addListener: vi.fn(),
	removeListener: vi.fn(),
};

beforeAll(() => {
	(globalThis as any).NativeModules = {
		NipworkerLynxModule: mockNativeModule,
	};
	(globalThis as any).lynx = {
		getJSModule: vi.fn((name: string) => {
			if (name === 'GlobalEventEmitter') return mockEmitter;
			return undefined;
		}),
	};
});

afterAll(() => {
	delete (globalThis as any).NativeModules;
	delete (globalThis as any).lynx;
});

function buildCryptoRawMessage(json: string): Uint8Array {
	const builder = new flatbuffers.Builder(256);
	const rawStr = builder.createString(json);
	const raw = Raw.createRaw(builder, rawStr);
	const msg = WorkerMessage.createWorkerMessage(
		builder,
		0,
		0,
		MessageType.Raw,
		Message.Raw,
		raw
	);
	builder.finish(msg);
	return builder.asUint8Array();
}

function buildNativeFrame(subId: string, payload: Uint8Array): Uint8Array {
	const subIdBytes = new TextEncoder().encode(subId);
	const frame = new Uint8Array(4 + subIdBytes.length + 4 + payload.length);
	const view = new DataView(frame.buffer);
	let offset = 0;
	view.setUint32(offset, subIdBytes.length, true);
	offset += 4;
	frame.set(subIdBytes, offset);
	offset += subIdBytes.length;
	view.setUint32(offset, payload.length, true);
	offset += 4;
	frame.set(payload, offset);
	return frame;
}

function bytesToBase64(bytes: Uint8Array): string {
	if (typeof btoa === 'function') {
		const binary = String.fromCharCode(...bytes);
		return btoa(binary);
	}
	// Node.js test environment fallback
	return Buffer.from(bytes).toString('base64');
}

describe('NativeBackend', () => {
	describe('GlobalEventEmitter listener', () => {
		it('should receive a base64 event, decode it, and route to handleCryptoMessage', () => {
			const backend = new NativeBackend();
			const authHandler = vi.fn();
			backend.addEventListener('auth', authHandler);

			// Capture the listener registered with GlobalEventEmitter
			expect(mockEmitter.addListener).toHaveBeenCalledWith('NipworkerEvent', expect.any(Function));
			const listener = mockEmitter.addListener.mock.calls[0][1];

			// Build a native frame: [subIdLen]["crypto"][payloadLen][cryptoPayload]
			const cryptoPayload = buildCryptoRawMessage(
				JSON.stringify({ op: 'get_public_key', result: 'deadbeef12345678deadbeef12345678deadbeef12345678deadbeef12345678' })
			);
			const frame = buildNativeFrame('crypto', cryptoPayload);
			const base64 = bytesToBase64(frame);

			// Simulate a Lynx global event arriving as an object envelope
			listener({ v: 1, encoding: 'base64', data: base64 });

			expect(authHandler).toHaveBeenCalledTimes(1);
			const detail = authHandler.mock.calls[0][0].detail;
			expect(detail.pubkey).toBe('deadbeef12345678deadbeef12345678deadbeef12345678deadbeef12345678');
			expect(detail.hasSigner).toBe(true);
		});

		it('should accept events wrapped in an array', () => {
			const backend = new NativeBackend();
			const authHandler = vi.fn();
			backend.addEventListener('auth', authHandler);

			const listener = mockEmitter.addListener.mock.calls[mockEmitter.addListener.mock.calls.length - 1][1];

			const cryptoPayload = buildCryptoRawMessage(
				JSON.stringify({ op: 'get_public_key', result: 'arraywrapped12345678arraywrapped12345678arraywrapped12345678arraywrapped12345678' })
			);
			const frame = buildNativeFrame('crypto', cryptoPayload);
			const base64 = bytesToBase64(frame);

			// Lynx sometimes passes params as an array
			listener([{ v: 1, encoding: 'base64', data: base64 }]);

			expect(authHandler).toHaveBeenCalledTimes(1);
			const detail = authHandler.mock.calls[0][0].detail;
			expect(detail.pubkey).toBe('arraywrapped12345678arraywrapped12345678arraywrapped12345678arraywrapped12345678');
		});

		it('should drop malformed envelopes', () => {
			const backend = new NativeBackend();
			const authHandler = vi.fn();
			backend.addEventListener('auth', authHandler);

			const listener = mockEmitter.addListener.mock.calls[mockEmitter.addListener.mock.calls.length - 1][1];

			// Wrong version
			listener({ v: 2, encoding: 'base64', data: 'abc' });
			// Wrong encoding
			listener({ v: 1, encoding: 'hex', data: 'abc' });
			// Missing data
			listener({ v: 1, encoding: 'base64' });

			expect(authHandler).not.toHaveBeenCalled();
		});
	});

	describe('handleCryptoMessage', () => {
		it('should dispatch auth event on set_signer success', () => {
			const backend = new NativeBackend();
			const authHandler = vi.fn();
			backend.addEventListener('auth', authHandler);

			// Simulate a pending privkey session
			(backend as any)._pendingSession = { type: 'privkey', payload: 'secret' };

			const payload = buildCryptoRawMessage(
				JSON.stringify({ op: 'set_signer', result: '79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798' })
			);
			(backend as any).handleCryptoMessage(payload);

			expect(authHandler).toHaveBeenCalledTimes(1);
			const detail = authHandler.mock.calls[0][0].detail;
			expect(detail.pubkey).toBe('79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798');
			expect(detail.hasSigner).toBe(true);
			expect(detail.secretKey).toBe('secret');
			expect((backend as any)._pendingSession).toBeNull();
			expect((backend as any).activePubkey).toBe('79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798');
		});

		it('should dispatch auth event on get_public_key success', () => {
			const backend = new NativeBackend();
			const authHandler = vi.fn();
			backend.addEventListener('auth', authHandler);

			const payload = buildCryptoRawMessage(
				JSON.stringify({ op: 'get_public_key', result: 'abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890' })
			);
			(backend as any).handleCryptoMessage(payload);

			expect(authHandler).toHaveBeenCalledTimes(1);
			const detail = authHandler.mock.calls[0][0].detail;
			expect(detail.pubkey).toBe('abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890');
			expect(detail.hasSigner).toBe(true);
		});

		it('should call _signCB on sign_event success', () => {
			const backend = new NativeBackend();
			const signHandler = vi.fn();
			(backend as any)._signCB = signHandler;

			const signedEvent = {
				id: 'event-id',
				pubkey: 'pubkey',
				created_at: 123,
				kind: 1,
				tags: [],
				content: 'hello',
				sig: 'sig',
			};
			const payload = buildCryptoRawMessage(
				JSON.stringify({ op: 'sign_event', result: JSON.stringify(signedEvent) })
			);
			(backend as any).handleCryptoMessage(payload);

			expect(signHandler).toHaveBeenCalledTimes(1);
			expect(signHandler).toHaveBeenCalledWith(signedEvent);
		});

		it('should dispatch auth with hasSigner=false on error response', () => {
			const backend = new NativeBackend();
			const authHandler = vi.fn();
			backend.addEventListener('auth', authHandler);

			const payload = buildCryptoRawMessage(
				JSON.stringify({ op: 'get_public_key', error: 'signer not available' })
			);
			(backend as any).handleCryptoMessage(payload);

			expect(authHandler).toHaveBeenCalledTimes(1);
			const detail = authHandler.mock.calls[0][0].detail;
			expect(detail.pubkey).toBeNull();
			expect(detail.hasSigner).toBe(false);
		});
	});
});
