import { ByteBuffer } from 'flatbuffers';

(ByteBuffer.prototype as any).__stringByteString = function (offset: number): ByteString {
	// Follow indirect: add the relative offset stored at this location
	offset += this.readInt32(offset);

	// Now at the start of the string object → first 4 bytes = length
	const length = this.readInt32(offset);
	const start = offset + 4;

	// Slice out exactly [start, start+length]
	const slice = this.bytes().subarray(start, start + length);

	return new ByteString(slice);
};

// Shared decoder for all instances (cheap, stateless for non-streaming use)
const UTF8_DECODER = new TextDecoder('utf-8');

// Hex lookup table to speed up toHex()
const HEX_TABLE: string[] = (() => {
	const t = new Array(256);
	for (let i = 0; i < 256; i++) t[i] = i.toString(16).padStart(2, '0');
	return t;
})();

// Convert Uint8Array to string using the browser's built-in decoder
function byteArrayToString(bytes: Uint8Array): string {
	return UTF8_DECODER.decode(bytes);
}

// lib/ByteString.ts
export class ByteString {
	private readonly view: Uint8Array;

	constructor(view: Uint8Array) {
		this.view = view;
	}

	/**
	 * Access underlying bytes
	 */
	bytes(): Uint8Array {
		return this.view;
	}

	/**
	 * Fast numeric discriminant (FNV-1a hash).
	 * Not cryptographically secure, but great for Map/Set keys.
	 */
	fnv1aHash(): number {
		let h = 2166136261 >>> 0;
		const v = this.view;
		for (let i = 0; i < v.length; i++) {
			h ^= v[i];
			h = Math.imul(h, 16777619);
		}
		return h >>> 0;
	}

	/**
	 * Return a stable hex string representation.
	 */
	toHex(): string {
		const v = this.view;
		let out = '';
		for (let i = 0; i < v.length; i++) out += HEX_TABLE[v[i]];
		return out;
	}

	/**
	 * Decode as UTF-8 string.
	 * If backed by SharedArrayBuffer, makes a safe copy.
	 * No caching to allow proper GC of event data.
	 */
	utf8String(): string {
		return byteArrayToString(this.view);
	}

	/**
	 * For debugging/logging
	 */
	toString(): string {
		return this.utf8String();
	}

	/**
	 * Equality check by bytes
	 */
	equals(other: ByteString): boolean {
		const a = this.view,
			b = other.view;
		if (a.length !== b.length) return false;
		for (let i = 0; i < a.length; i++) {
			if (a[i] !== b[i]) return false;
		}
		return true;
	}
}
