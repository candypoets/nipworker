import { describe, it, expect, vi, beforeAll, afterAll } from 'vitest';
import { NativeBackend } from './NativeBackend';
import * as flatbuffers from 'flatbuffers';
import { WorkerMessage, MessageType, Raw, Message } from './generated/nostr/fb';

const mockNativeModule = {
	init: vi.fn((cb: any) => {
		// store callback for later use if needed
	}),
	handleMessage: vi.fn(),
	setPrivateKey: vi.fn(),
	deinit: vi.fn(),
};

beforeAll(() => {
	(globalThis as any).NativeModules = {
		NipworkerLynxModule: mockNativeModule,
	};
});

afterAll(() => {
	delete (globalThis as any).NativeModules;
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

describe('NativeBackend', () => {
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
