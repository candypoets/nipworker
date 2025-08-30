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
    for (let i = 0; i < this.view.length; i++) {
      h ^= this.view[i];
      h = Math.imul(h, 16777619);
    }
    return h >>> 0;
  }

  /**
   * Return a stable hex string representation.
   */
  toHex(): string {
    return Array.from(this.view)
      .map(b => b.toString(16).padStart(2, "0"))
      .join("");
  }

  /**
   * Decode as UTF-8 string.
   * If backed by SharedArrayBuffer, makes a safe copy.
   */
  utf8String(): string {
    if (this._utf8 !== undefined) return this._utf8;

       if (this.view.buffer instanceof SharedArrayBuffer) {
         const copy = new Uint8Array(this.view);
         return (this._utf8 = new TextDecoder("utf-8").decode(copy));
       }
       return (this._utf8 = new TextDecoder("utf-8").decode(this.view));
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
    if (this.view.length !== other.view.length) return false;
    for (let i = 0; i < this.view.length; i++) {
      if (this.view[i] !== other.view[i]) return false;
    }
    return true;
  }
}
