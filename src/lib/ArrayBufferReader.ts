import { ByteBuffer } from 'flatbuffers';
import { WorkerMessage } from 'src/generated/nostr/fb';

/**
 * Initial size for elastic subscription buffers. Buffers grow on demand
 * (doubling) up to the hard cap computed by `calculateBufferSize`.
 */
export const INITIAL_SUBSCRIPTION_BUFFER_SIZE = 256 * 1024;

// Runtime feature-detect (once): resizable ArrayBuffers (ES2024) let
// subscription buffers start small and grow on demand. Engines without
// `ArrayBuffer.prototype.resize` fall back to fixed full-cap allocation.
const supportsResizableArrayBuffer =
	typeof (ArrayBuffer.prototype as { resize?: unknown }).resize === 'function';

// Narrow structural type for the ES2024 resizable-ArrayBuffer API; the
// tsconfig lib target (ES2020) predates it, so casts go through this.
interface ResizableArrayBuffer extends ArrayBuffer {
	resizable: boolean;
	maxByteLength: number;
	resize(newByteLength: number): void;
}

const ResizableArrayBufferCtor = ArrayBuffer as unknown as new (
	byteLength: number,
	options?: { maxByteLength?: number }
) => ArrayBuffer;

/**
 * Utility library for reading from ArrayBuffer with 4-byte header approach
 * Header format: [0-3]: Write position (4 bytes, little endian)
 * Data format: [4+]: [4-byte length][FlatBuffer message][4-byte length][FlatBuffer message]...
 * 
 * Utility for reading from ArrayBuffer with 4-byte header approach.
 * Used for MessageChannel-based communication between main thread and workers.
 */
export class ArrayBufferReader {
	static malformedFrameLogOnceBySubId = new Set<string>();
	/**
	 * Initialize a buffer for writing - sets the write position header to 4
	 * @param buffer The ArrayBuffer to initialize
	 */
	static initializeBuffer(buffer: ArrayBuffer): void {
		const view = new DataView(buffer);
		// Set initial write position to 4 (right after the header)
		view.setUint32(0, 4, true);
	}

	/**
	 * Allocate a subscription buffer. When resizable ArrayBuffers are supported
	 * and the cap exceeds the initial size, the buffer starts small and grows
	 * on demand (see writePayload) up to `cap`; otherwise it is a fixed
	 * full-cap buffer, exactly as before.
	 * @param cap Hard upper bound in bytes (from calculateBufferSize)
	 * @param initialSize Starting size when elastic (defaults to INITIAL_SUBSCRIPTION_BUFFER_SIZE)
	 */
	static createBuffer(
		cap: number,
		initialSize: number = INITIAL_SUBSCRIPTION_BUFFER_SIZE
	): ArrayBuffer {
		if (supportsResizableArrayBuffer && initialSize < cap) {
			return new ResizableArrayBufferCtor(initialSize, { maxByteLength: cap });
		}
		return new ArrayBuffer(cap);
	}

	/**
	 * Grow a resizable buffer to fit `requiredSize` (doubling, clamped to
	 * maxByteLength). Returns false when the buffer cannot grow (fixed buffer,
	 * no engine support, or already at cap).
	 */
	private static growBuffer(buffer: ArrayBuffer, requiredSize: number): boolean {
		if (!supportsResizableArrayBuffer) return false;
		const rb = buffer as Partial<ResizableArrayBuffer>;
		if (typeof rb.resize !== 'function' || typeof rb.maxByteLength !== 'number') return false;
		if (buffer.byteLength >= rb.maxByteLength) return false;
		const next = Math.min(rb.maxByteLength, Math.max(buffer.byteLength * 2, requiredSize));
		if (next <= buffer.byteLength) return false;
		rb.resize(next);
		return true;
	}

	/**
	 * Write a message to the ArrayBuffer
	 * @param buffer The ArrayBuffer to write to
	 * @param data The data to write (already encoded as Uint8Array)
	 * @returns True if written successfully, false if buffer is full
	 */
	static writeMessage(buffer: ArrayBuffer, data: Uint8Array): boolean {
		const view = new DataView(buffer);
		const uint8View = new Uint8Array(buffer);

		// Get current write position
		const currentWritePosition = view.getUint32(0, true);

		// Check if there's enough space (4 bytes for length + data length)
		const requiredSpace = 4 + data.length;
		if (currentWritePosition + requiredSpace > buffer.byteLength) {
			console.warn('Buffer full, cannot write message');
			return false;
		}

		// Write the length prefix (4 bytes, little endian)
		view.setUint32(currentWritePosition, data.length, true);

		// Write the actual data
		uint8View.set(data, currentWritePosition + 4);

		// Update the write position header
		const newWritePosition = currentWritePosition + requiredSpace;
		view.setUint32(0, newWritePosition, true);

		return true;
	}

	/**
	 * Write raw batched data (already length-prefixed) to the ArrayBuffer.
	 * Use this when the data already contains [4-byte len][payload] format.
	 * @param buffer The ArrayBuffer to write to
	 * @param data The batched data (already with length prefixes)
	 * @returns True if written successfully, false if buffer is full
	 */
	static writeBatchedData(buffer: ArrayBuffer, data: Uint8Array, _debugId?: string): boolean {
		const view = new DataView(buffer);
		const uint8View = new Uint8Array(buffer);

		// Get current write position
		const currentWritePosition = view.getUint32(0, true);

		// Check if there's enough space
		if (currentWritePosition + data.length > buffer.byteLength) {
			console.warn(
				`[ArrayBufferReader] Dropping ${_debugId ? `event for subscription '${_debugId}'` : 'event'}: ` +
				`buffer full (${currentWritePosition}/${buffer.byteLength} bytes used, ` +
				`need ${data.length} more bytes). ` +
				`Consider increasing 'bytesPerEvent' or reducing subscription limits.`
			);
			return false;
		}

		// Write the data directly (it's already length-prefixed)
		uint8View.set(data, currentWritePosition);

		// Update the write position header
		const newWritePosition = currentWritePosition + data.length;
		view.setUint32(0, newWritePosition, true);

		return true;
	}

	static writePayload(buffer: ArrayBuffer, payload: Uint8Array, _debugId?: string): boolean {
		const requiredSpace = 4 + payload.length;
		if (this.tryWritePayload(buffer, payload)) return true;

		// Buffer full: grow it if possible and retry once. The cap
		// (maxByteLength) is still the hard bound — a write that does not fit
		// even at cap reports full exactly as a fixed buffer would.
		const currentWritePosition = this.getCurrentWritePosition(buffer);
		if (
			this.growBuffer(buffer, currentWritePosition + requiredSpace) &&
			this.tryWritePayload(buffer, payload)
		) {
			return true;
		}

		console.warn(
			`[ArrayBufferReader] Dropping ${_debugId ? `event for subscription '${_debugId}'` : 'event'}: ` +
				`buffer full (${currentWritePosition}/${buffer.byteLength} bytes used, ` +
				`need ${requiredSpace} more bytes). ` +
				`Consider increasing 'bytesPerEvent' or reducing subscription limits.`
		);
		return false;
	}

	private static tryWritePayload(buffer: ArrayBuffer, payload: Uint8Array): boolean {
		const view = new DataView(buffer);
		const uint8View = new Uint8Array(buffer);
		const currentWritePosition = view.getUint32(0, true);
		const requiredSpace = 4 + payload.length;

		if (currentWritePosition + requiredSpace > buffer.byteLength) {
			return false;
		}

		view.setUint32(currentWritePosition, payload.length, true);
		uint8View.set(payload, currentWritePosition + 4);
		view.setUint32(0, currentWritePosition + requiredSpace, true);
		return true;
	}

	/**
	 * Read new messages from ArrayBuffer since last read position
	 * @param buffer The ArrayBuffer to read from
	 * @param lastReadPosition Last position read (default: 0, meaning read from beginning)
	 * @returns Object containing new messages and updated read position
	 */
	static readMessages(
		buffer: ArrayBuffer,
		lastReadPosition: number = 0,
		debugId?: string
	) {
		const view = new DataView(buffer);
		const uint8View = new Uint8Array(buffer);

		const currentWritePosition = view.getUint32(0, true);
		const dataStartOffset = 4;
		let currentPos = lastReadPosition < dataStartOffset ? dataStartOffset : lastReadPosition;
		if (currentWritePosition <= currentPos) {
			return { messages: [], newReadPosition: currentPos, hasNewData: false };
		}

		const maxMessages = 128;
		const messages: WorkerMessage[] = new Array(maxMessages);
		let msgCount = 0;

		try {
			while (currentPos < currentWritePosition) {
				// Stop if we've filled this batch; leave currentPos at the start of the next message
				if (msgCount >= maxMessages) break;

				// Need enough bytes for header
				if (currentPos + 4 > currentWritePosition) break;

				const headerPos = currentPos;
				const eventLength = view.getUint32(headerPos, true);
				const payloadStart = headerPos + 4;
				const nextPos = payloadStart + eventLength;

				// Need the full payload to be available
				if (nextPos > currentWritePosition) {
					const cacheKey = debugId || '__no_sub_id__';
					if (!this.malformedFrameLogOnceBySubId.has(cacheKey)) {
						this.malformedFrameLogOnceBySubId.add(cacheKey);
						console.error(
							'[ArrayBufferReader] malformed frame, skipping remaining bytes: ' +
								`subId=${debugId ?? 'unknown'}, ` +
								`subIdLen=${debugId ? debugId.length : -1}, ` +
								`payloadLen=${eventLength}, ` +
								`currentWritePosition=${currentWritePosition}, ` +
								`lastReadPos=${lastReadPosition}`
						);
					}
					currentPos = currentWritePosition;
					break;
				}

				// Parse directly from the existing buffer view to avoid per-message copies.
				const bb = new ByteBuffer(uint8View.subarray(payloadStart, nextPos));
				const message = WorkerMessage.getRootAsWorkerMessage(bb);

				messages[msgCount++] = message;

				// Advance to next message boundary
				currentPos = nextPos;
			}

			messages.length = msgCount;
			return {
				messages,
				newReadPosition: currentPos,
				hasNewData: msgCount > 0
			};
		} catch (error) {
			console.error('Failed to decode FlatBuffer data from ArrayBuffer:', error);
			messages.length = msgCount;
			return {
				messages,
				newReadPosition:
					lastReadPosition < dataStartOffset ? dataStartOffset : lastReadPosition,
				hasNewData: false
			};
		}
	}

	/**
	 * Read all messages from ArrayBuffer from the beginning (ignores lastReadPosition)
	 * @param buffer The ArrayBuffer to read from
	 * @returns Object containing all messages in the buffer
	 */
	static readAllMessages(buffer: ArrayBuffer): {
		messages: WorkerMessage[];
		totalMessages: number;
	} {
		const result = this.readMessages(buffer, 0); // Always start from beginning
		return {
			messages: result.messages,
			totalMessages: result.messages.length
		};
	}

	/**
	 * Get current write position from buffer header
	 * @param buffer The ArrayBuffer to read from
	 * @returns Current write position
	 */
	static getCurrentWritePosition(buffer: ArrayBuffer): number {
		const view = new DataView(buffer);
		return view.getUint32(0, true);
	}

	/**
	 * Check if buffer has new data since last read
	 * @param buffer The ArrayBuffer to check
	 * @param lastReadPosition Last position read
	 * @returns True if there's new data to read
	 */
	static hasNewData(buffer: ArrayBuffer, lastReadPosition: number): boolean {
		const currentWritePosition = this.getCurrentWritePosition(buffer);
		const dataStartOffset = 4;
		const actualLastReadPosition = Math.max(lastReadPosition, dataStartOffset);
		return currentWritePosition > actualLastReadPosition;
	}

	/**
	 * Calculate recommended buffer size based on request limits
	 * @param totalEventLimit Total expected events across all requests
	 * @param bytesPerEvent Estimated bytes per event (default: 3072)
	 * @returns Recommended buffer size in bytes
	 */
	static calculateBufferSize(totalEventLimit: number = 100, bytesPerEvent: number = 3072): number {
		const headerSize = 4;
		const dataSize = totalEventLimit * bytesPerEvent;
		const overhead = Math.floor(dataSize * 0.25); // 25% overhead
		return headerSize + dataSize + overhead;
	}
}
