import { describe, expect, it } from 'vitest';
import * as flatbuffers from 'flatbuffers';

import { ArrayBufferReader } from './ArrayBufferReader';
import { Eoce, Message, MessageType, WorkerMessage } from '../generated/nostr/fb';

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
});
