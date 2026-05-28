import { describe, expect, it } from 'vitest';

import { ArrayBufferReader } from './ArrayBufferReader';

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
});
