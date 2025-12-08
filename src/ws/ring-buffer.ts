export class ByteRingBuffer {
	private readonly sab: SharedArrayBuffer;
	private readonly dataView: DataView;
	private readonly dataStart: number = 32;
	private readonly capacity: number;
	private dropped: number = 0;

	constructor(buffer: SharedArrayBuffer) {
		this.sab = buffer;
		this.dataView = new DataView(buffer);
		this.capacity = this.dataView.getUint32(0, true); // little-endian

		// Read initial state (assume Rust sets capacity, head=0, tail=0, seq=0)
		// We don't reset here; assume initialized
	}

	private getHead(): number {
		return this.dataView.getUint32(4, true) % this.capacity;
	}

	private setHead(value: number): void {
		this.dataView.setUint32(4, value % this.capacity, true);
	}

	private getTail(): number {
		return this.dataView.getUint32(8, true) % this.capacity;
	}

	private setTail(value: number): void {
		this.dataView.setUint32(8, value % this.capacity, true);
	}

	private getSeq(): number {
		return this.dataView.getUint32(12, true);
	}

	private setSeq(value: number): void {
		this.dataView.setUint32(12, value, true);
	}

	getFreeSpace(): number {
		const head = this.getHead();
		const tail = this.getTail();
		const used = (head - tail + this.capacity) % this.capacity;
		return this.capacity - used;
	}

	getDropped(): number {
		return this.dropped;
	}

	hasRecords(): boolean {
		return this.getHead() !== this.getTail();
	}

	readNext(): { payload: Uint8Array } | null {
		const p = this.read();
		return p ? { payload: p } : null;
	}

	/**
	 * Writes a payload to the ring buffer, overwriting old records if necessary.
	 * Returns the sequence number if successful, -1 if dropped (couldn't make space).
	 */
	write(payload: Uint8Array): number {
		const N = payload.byteLength;
		const len = 8 + N; // type(2) + pad(2) + seq(4) + payload(N)
		const totalSize = 4 + len + 4; // len + variable + trailer

		// Make space by skipping records
		let droppedThisWrite = 0;
		while (this.getFreeSpace() < totalSize) {
			if (!this.skipRecord()) {
				// Can't skip more (uncommitted record), drop this write
				this.dropped += droppedThisWrite + 1;
				return -1;
			}
			droppedThisWrite++;
		}

		// Enough space, write
		const mySeq = this.getSeq() + 1;
		this.setSeq(mySeq);

		let writePos = this.getHead();

		// Write len
		this.dataView.setUint32(this.dataStart + writePos, len, true);
		writePos = (writePos + 4) % this.capacity;

		// Write type (0)
		this.dataView.setUint16(this.dataStart + writePos, 0, true);
		writePos = (writePos + 2) % this.capacity;

		// Write pad (0)
		this.dataView.setUint16(this.dataStart + writePos, 0, true);
		writePos = (writePos + 2) % this.capacity;

		// Write seq
		this.dataView.setUint32(this.dataStart + writePos, mySeq, true);
		writePos = (writePos + 4) % this.capacity;

		// Write payload (possibly wrapped)
		this.copyBytes(writePos, payload, 0, N);
		writePos = (writePos + N) % this.capacity;

		// Write trailer (len)
		this.dataView.setUint32(this.dataStart + writePos, len, true);
		writePos = (writePos + 4) % this.capacity;

		// Advance head
		this.setHead(writePos);

		this.dropped += droppedThisWrite;
		return mySeq;
	}

	/**
	 * Reads the next committed payload or null if none ready.
	 * Advances tail on success.
	 */
	read(): Uint8Array | null {
		let readPos = this.getTail();
		if (readPos === this.getHead()) return null; // empty

		const len = this.dataView.getUint32(this.dataStart + readPos, true);
		if (len === 0) return null;

		const trailerPos = (readPos + 4 + len) % this.capacity;
		const trailer = this.dataView.getUint32(this.dataStart + trailerPos, true);

		if (trailer !== len) return null; // not committed

		// Read variable part (len bytes from readPos + 4)
		const variable = new Uint8Array(len);
		this.copyFromRing((readPos + 4) % this.capacity, variable, 0, len);

		// Extract payload (skip type+pad+seq = 8 bytes)
		const payload = variable.subarray(8);

		// Advance tail
		const advance = 4 + len + 4; // len field + variable + trailer
		this.setTail((this.getTail() + advance) % this.capacity);

		return payload;
	}

	private skipRecord(): boolean {
		let readPos = this.getTail();
		if (readPos === this.getHead()) return false; // empty

		const len = this.dataView.getUint32(this.dataStart + readPos, true);
		if (len === 0) return false;

		const trailerPos = (readPos + 4 + len) % this.capacity;
		const trailer = this.dataView.getUint32(this.dataStart + trailerPos, true);

		if (trailer !== len) return false; // not committed

		// Skip by advancing tail
		const advance = 4 + len + 4;
		this.setTail((this.getTail() + advance) % this.capacity);
		return true;
	}

	private copyBytes(
		targetPos: number,
		source: Uint8Array,
		sourceOffset: number,
		length: number
	): void {
		let remaining = length;
		let srcOffset = sourceOffset;
		let tgt = targetPos;

		while (remaining > 0) {
			const spaceToEnd = this.capacity - (tgt % this.capacity);
			const chunkSize = Math.min(remaining, spaceToEnd);
			const tgtAbs = this.dataStart + (tgt % this.capacity);
			const srcChunk = source.subarray(srcOffset, srcOffset + chunkSize);
			new Uint8Array(this.sab, tgtAbs, chunkSize).set(srcChunk);
			remaining -= chunkSize;
			srcOffset += chunkSize;
			tgt += chunkSize;
		}
	}

	private copyFromRing(
		sourcePos: number,
		target: Uint8Array,
		targetOffset: number,
		length: number
	): void {
		let remaining = length;
		let tgtOffset = targetOffset;
		let src = sourcePos;

		while (remaining > 0) {
			const spaceToEnd = this.capacity - (src % this.capacity);
			const chunkSize = Math.min(remaining, spaceToEnd);
			const srcAbs = this.dataStart + (src % this.capacity);
			const srcChunk = new Uint8Array(this.sab, srcAbs, chunkSize);
			target.set(srcChunk, tgtOffset);
			remaining -= chunkSize;
			tgtOffset += chunkSize;
			src += chunkSize;
		}
	}
}
