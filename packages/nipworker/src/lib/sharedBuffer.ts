import { encode } from "@msgpack/msgpack";
import { unpack } from "msgpackr";
import type { WorkerToMainMessage } from "src/types";

/**
 * Utility library for reading from SharedArrayBuffer with 4-byte header approach
 * Header format: [0-3]: Write position (4 bytes, little endian)
 * Data format: [4+]: [4-byte length][msgpack event][4-byte length][msgpack event]...
 */
export class SharedBufferReader {
  /**
     * Initialize a buffer for writing - sets the write position header to 4
     * @param buffer The SharedArrayBuffer to initialize
     */
    static initializeBuffer(buffer: SharedArrayBuffer): void {
      const view = new DataView(buffer);
      // Set initial write position to 4 (right after the header)
      view.setUint32(0, 4, true);
    }

    /**
     * Write a message to the SharedArrayBuffer
     * @param buffer The SharedArrayBuffer to write to
     * @param data The data to write (already encoded as Uint8Array)
     * @returns True if written successfully, false if buffer is full
     */
    static writeMessage(buffer: SharedArrayBuffer, data: Uint8Array): boolean {
      const view = new DataView(buffer);
      const uint8View = new Uint8Array(buffer);

      // Get current write position
      const currentWritePosition = view.getUint32(0, true);

      // Check if there's enough space (4 bytes for length + data length)
      const requiredSpace = 4 + data.length;
      if (currentWritePosition + requiredSpace > buffer.byteLength) {
        console.warn("Buffer full, cannot write message");
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
     * Write a message object to the SharedArrayBuffer (handles encoding)
     * @param buffer The SharedArrayBuffer to write to
     * @param message The message object to write
     * @returns True if written successfully, false if buffer is full
     */
    static writeMessageObject(buffer: SharedArrayBuffer, message: any): boolean {
      try {
        const encoded = encode(message);
        return this.writeMessage(buffer, new Uint8Array(encoded));
      } catch (error) {
        console.error("Failed to encode message:", error);
        return false;
      }
    }
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

        // Check for special "buffer full" marker
        if (eventLength === 1) {
          if (currentPos + 1 <= currentWritePosition) {
            const marker = uint8View[currentPos];
            if (marker === 0xFF) {
              // Buffer full detected - create special message
              const bufferFullMessage: WorkerToMainMessage = {
                SubscriptionEvent: {
                  subscription_id: "",
                  event_type: "BUFFER_FULL" as any,
                  event_data: []
                }
              };
              messages.push(bufferFullMessage);
              currentPos += 1;
              continue;
            }
          }
        }

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
