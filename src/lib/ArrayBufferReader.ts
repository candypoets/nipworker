import { ByteBuffer } from 'flatbuffers';
import { WorkerMessage } from 'src/generated/nostr/fb';

/**
 * Utility library for reading from ArrayBuffer with 4-byte header approach
 * Header format: [0-3]: Write position (4 bytes, little endian)
 * Data format: [4+]: [4-byte length][FlatBuffer message][4-byte length][FlatBuffer message]...
 * 
 * Similar to SharedBufferReader but works with regular ArrayBuffer instead of SharedArrayBuffer.
 * Used for MessageChannel-based communication between main thread and workers.
 */
export class ArrayBufferReader {
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
	 * Read new messages from ArrayBuffer since last read position
	 * @param buffer The ArrayBuffer to read from
	 * @param lastReadPosition Last position read (default: 0, meaning read from beginning)
	 * @returns Object containing new messages and updated read position
	 */
	static readMessages(buffer: ArrayBuffer, lastReadPosition: number = 0) {
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
				if (nextPos > currentWritePosition) break;

				// Create a copy of the data since ArrayBuffer is not shared
				const flatBufferData = new Uint8Array(uint8View.subarray(payloadStart, nextPos));

				// Parse directly
				const bb = new ByteBuffer(flatBufferData);
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
				newReadPosition: lastReadPosition < dataStartOffset ? dataStartOffset : lastReadPosition,
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
