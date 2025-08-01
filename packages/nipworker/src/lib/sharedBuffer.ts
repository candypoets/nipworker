import { unpack } from "msgpackr";
import type { WorkerToMainMessage } from "src/types";

/**
 * Utility library for reading from SharedArrayBuffer with 4-byte header approach
 * Header format: [0-3]: Write position (4 bytes, little endian)
 * Data format: [4+]: [4-byte length][msgpack event][4-byte length][msgpack event]...
 */
export class SharedBufferReader {
  /**
   * Read new messages from SharedArrayBuffer since last read position
   * @param buffer The SharedArrayBuffer to read from
   * @param lastReadPosition Last position read (default: 0, meaning read from beginning)
   * @returns Object containing new messages and updated read position
   */
  static readMessages(
    buffer: SharedArrayBuffer,
    lastReadPosition: number = 0,
  ): {
    messages: WorkerToMainMessage[];
    newReadPosition: number;
    hasNewData: boolean;
  } {
    const view = new DataView(buffer);
    const uint8View = new Uint8Array(buffer);

    // Read current write position from header (first 4 bytes, little endian)
    const currentWritePosition = view.getUint32(0, true);

    // Check if there's new data to read
    const dataStartOffset = 4; // Skip 4-byte write position header
    const actualLastReadPosition = Math.max(lastReadPosition, dataStartOffset);

    if (currentWritePosition <= actualLastReadPosition) {
      return {
        messages: [],
        newReadPosition: actualLastReadPosition,
        hasNewData: false,
      };
    }

    const messages: WorkerToMainMessage[] = [];
    let currentPos = actualLastReadPosition;

    try {
      // Read length-prefixed events
      while (currentPos < currentWritePosition) {
        // Read 4-byte length prefix (little endian)
        if (currentPos + 4 > currentWritePosition) break;
        const eventLength = view.getUint32(currentPos, true);
        currentPos += 4;

        // Read the event data
        if (currentPos + eventLength > currentWritePosition) break;
        const eventData = uint8View.slice(currentPos, currentPos + eventLength);

        // Decode the event
        const message = unpack(eventData) as WorkerToMainMessage;
        messages.push(message);

        currentPos += eventLength;
      }

      return {
        messages,
        newReadPosition: currentPos,
        hasNewData: messages.length > 0,
      };
    } catch (error) {
      console.error(
        "Failed to decode length-prefixed msgpack data from SharedArrayBuffer:",
        error,
      );
      return {
        messages,
        newReadPosition: actualLastReadPosition,
        hasNewData: false,
      };
    }
  }

  /**
   * Read all messages from SharedArrayBuffer from the beginning (ignores lastReadPosition)
   * @param buffer The SharedArrayBuffer to read from
   * @returns Object containing all messages in the buffer
   */
  static readAllMessages(buffer: SharedArrayBuffer): {
    messages: WorkerToMainMessage[];
    totalMessages: number;
  } {
    const result = this.readMessages(buffer, 0); // Always start from beginning
    return {
      messages: result.messages,
      totalMessages: result.messages.length,
    };
  }

  /**
   * Get current write position from buffer header
   * @param buffer The SharedArrayBuffer to read from
   * @returns Current write position
   */
  static getCurrentWritePosition(buffer: SharedArrayBuffer): number {
    const view = new DataView(buffer);
    return view.getUint32(0, true);
  }

  /**
   * Check if buffer has new data since last read
   * @param buffer The SharedArrayBuffer to check
   * @param lastReadPosition Last position read
   * @returns True if there's new data to read
   */
  static hasNewData(
    buffer: SharedArrayBuffer,
    lastReadPosition: number,
  ): boolean {
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
  static calculateBufferSize(
    totalEventLimit: number = 100,
    bytesPerEvent: number = 3072,
  ): number {
    const headerSize = 4;
    const dataSize = totalEventLimit * bytesPerEvent;
    const overhead = Math.floor(dataSize * 0.25); // 25% overhead
    return headerSize + dataSize + overhead;
  }
}
