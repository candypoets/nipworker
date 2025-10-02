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

// Cross-instance cache: per underlying buffer → (offset:length) → decoded string
// WeakMap ensures entries are eligible for GC once the ArrayBuffer becomes unreachable.
const BUFFER_STRING_CACHE = new WeakMap<ArrayBufferLike, Map<string, string>>();

// Hex lookup table to speed up toHex()
const HEX_TABLE: string[] = (() => {
	const t = new Array(256);
	for (let i = 0; i < 256; i++) t[i] = i.toString(16).padStart(2, '0');
	return t;
})();

// lib/ByteString.ts
export class ByteString {
	private readonly view: Uint8Array;
	private _utf8?: string;

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
	 * Optimizations:
	 * - Reuses a shared TextDecoder
	 * - ASCII fast path
	 * - Cross-instance cache (non-SAB only) keyed by buffer+offset+length
	 */
	utf8String(): string {
		if (this._utf8 !== undefined) return this._utf8;

		const v = this.view;

		// 1) ASCII fast path (quick OR-scan for any high bit)
		let acc = 0;
		for (let i = 0; i < v.length; i++) acc |= v[i];
		if ((acc & 0x80) === 0) {
			// All ASCII → decode via String.fromCharCode in chunks
			let s = '';
			const CHUNK = 0x8000; // 32k is safe for apply()
			for (let i = 0; i < v.length; i += CHUNK) {
				s += String.fromCharCode.apply(null, v.subarray(i, i + CHUNK) as unknown as number[]);
			}
			return (this._utf8 = s);
		}

		// 2) Non-ASCII: try cross-instance cache (non-SAB only)
		const buf = v.buffer;
		if (!(buf instanceof SharedArrayBuffer)) {
			let cache = BUFFER_STRING_CACHE.get(buf);
			if (!cache) {
				cache = new Map<string, string>();
				BUFFER_STRING_CACHE.set(buf, cache);
			}
			const key = `${v.byteOffset}:${v.byteLength}`;
			const cached = cache.get(key);
			if (cached !== undefined) return (this._utf8 = cached);

			const s = UTF8_DECODER.decode(v);
			cache.set(key, s);
			return (this._utf8 = s);
		}

		// 3) SAB path: copy then decode, avoid cross-instance cache by default
		const safeCopy = new Uint8Array(v);
		const s = UTF8_DECODER.decode(safeCopy);
		return (this._utf8 = s);
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
