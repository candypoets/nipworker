import { describe, expect, it, vi } from 'vitest';
import * as flatbuffers from 'flatbuffers';

import { ArrayBufferReader } from './ArrayBufferReader';
import { Eoce, Message, MessageType, WorkerMessage } from '../generated/nostr/fb';

const hasResizableArrayBuffer =
	typeof (ArrayBuffer.prototype as { resize?: unknown }).resize === 'function';

function buildEoceMessage(subId: string): Uint8Array {
	const builder = new flatbuffers.Builder(256);
	const subscriptionId = builder.createString(subId);
	const eoce = Eoce.createEoce(builder, subscriptionId);
	const message = WorkerMessage.createWorkerMessage(
		builder,
		0,
		0,
		MessageType.Eoce,
		Message.Eoce,
		eoce
	);
	builder.finish(message);
	return builder.asUint8Array();
}

describe('ArrayBufferReader', () => {
	it('writes a raw payload with a length prefix', () => {
		const buffer = new ArrayBuffer(32);
		ArrayBufferReader.initializeBuffer(buffer);

		const payload = new Uint8Array([1, 2, 3, 4, 5]);
		expect(ArrayBufferReader.writePayload(buffer, payload)).toBe(true);

		const view = new DataView(buffer);
		const bytes = new Uint8Array(buffer);
		expect(view.getUint32(0, true)).toBe(13);
		expect(view.getUint32(4, true)).toBe(payload.length);
		expect(Array.from(bytes.slice(8, 13))).toEqual([1, 2, 3, 4, 5]);
	});

	it('does not advance the write position when the buffer is full', () => {
		const buffer = new ArrayBuffer(8);
		ArrayBufferReader.initializeBuffer(buffer);

		expect(ArrayBufferReader.writePayload(buffer, new Uint8Array([1]))).toBe(false);
		expect(ArrayBufferReader.getCurrentWritePosition(buffer)).toBe(4);
	});

	it('reads WorkerMessage FlatBuffers written through writePayload', () => {
		const buffer = new ArrayBuffer(256);
		ArrayBufferReader.initializeBuffer(buffer);

		expect(ArrayBufferReader.writePayload(buffer, buildEoceMessage('turbo-sub'))).toBe(true);

		const result = ArrayBufferReader.readMessages(buffer, 4);
		expect(result.hasNewData).toBe(true);
		expect(result.messages).toHaveLength(1);
		expect(result.newReadPosition).toBe(ArrayBufferReader.getCurrentWritePosition(buffer));

		const message = result.messages[0]!;
		expect(message.type()).toBe(MessageType.Eoce);
		expect(message.contentType()).toBe(Message.Eoce);
		expect(message.content(new Eoce())?.subscriptionId()).toBe('turbo-sub');
	});

	it('createBuffer allocates elastically with the cap as maxByteLength', () => {
		const buffer = ArrayBufferReader.createBuffer(4096, 64);
		if (hasResizableArrayBuffer) {
			expect(buffer.byteLength).toBe(64);
			expect((buffer as { maxByteLength?: number }).maxByteLength).toBe(4096);
		} else {
			// Fallback: fixed full-cap allocation
			expect(buffer.byteLength).toBe(4096);
		}
	});

	it('grows on write past the initial size and keeps written data intact', () => {
		if (!hasResizableArrayBuffer) return;
		const buffer = ArrayBufferReader.createBuffer(4096, 64);
		ArrayBufferReader.initializeBuffer(buffer);

		const first = buildEoceMessage('sub-a');
		expect(ArrayBufferReader.writePayload(buffer, first)).toBe(true);
		// Read the first message and keep the cursor across the resize.
		const initialRead = ArrayBufferReader.readMessages(buffer, 4);
		expect(initialRead.hasNewData).toBe(true);

		// Write past the initial 64 bytes: forces at least one resize.
		let wrote = 0;
		while (ArrayBufferReader.getCurrentWritePosition(buffer) < 200) {
			expect(ArrayBufferReader.writePayload(buffer, buildEoceMessage(`sub-${wrote}`))).toBe(
				true
			);
			wrote++;
		}
		expect(buffer.byteLength).toBeGreaterThan(64);
		expect(buffer.byteLength).toBeLessThanOrEqual(4096);

		// The pre-resize cursor still points at valid data after the resize.
		const rest = ArrayBufferReader.readMessages(buffer, initialRead.newReadPosition);
		expect(rest.messages).toHaveLength(wrote);
		expect(rest.messages[0]!.content(new Eoce())?.subscriptionId()).toBe('sub-0');
		expect(rest.messages[wrote - 1]!.content(new Eoce())?.subscriptionId()).toBe(
			`sub-${wrote - 1}`
		);

		// The full stream is intact when read from the beginning.
		const all = ArrayBufferReader.readAllMessages(buffer);
		expect(all.totalMessages).toBe(wrote + 1);
		expect(all.messages[0]!.content(new Eoce())?.subscriptionId()).toBe('sub-a');
	});

	it('still reports full once the cap is reached', () => {
		const buffer = ArrayBufferReader.createBuffer(128, 64);
		ArrayBufferReader.initializeBuffer(buffer);

		// 24 bytes per frame (4-byte length + 20-byte payload): 5 frames fit
		// within the 128-byte cap, the 6th must fail.
		const payload = new Uint8Array(20);
		for (let i = 0; i < 5; i++) {
			expect(ArrayBufferReader.writePayload(buffer, payload)).toBe(true);
		}
		expect(buffer.byteLength).toBeLessThanOrEqual(128);
		expect(ArrayBufferReader.writePayload(buffer, payload)).toBe(false);
		// The failed write does not advance the write position.
		expect(ArrayBufferReader.getCurrentWritePosition(buffer)).toBe(124);
	});

	it('falls back to a fixed full-cap buffer when resize is unavailable', async () => {
		const proto = ArrayBuffer.prototype as { resize?: unknown };
		const original = proto.resize;
		proto.resize = undefined;
		try {
			vi.resetModules();
			const { ArrayBufferReader: FreshReader } = await import('./ArrayBufferReader');
			const buffer = FreshReader.createBuffer(128, 64);
			expect(buffer.byteLength).toBe(128);
			expect((buffer as { resizable?: boolean }).resizable).toBeFalsy();

			FreshReader.initializeBuffer(buffer);
			const payload = new Uint8Array(20);
			for (let i = 0; i < 5; i++) {
				expect(FreshReader.writePayload(buffer, payload)).toBe(true);
			}
			expect(FreshReader.writePayload(buffer, payload)).toBe(false);
		} finally {
			proto.resize = original;
			vi.resetModules();
		}
	});

});
