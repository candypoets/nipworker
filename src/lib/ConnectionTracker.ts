import { ConnectionStatus, WorkerMessage } from "src/generated/nostr/fb";
import { isConnectionStatus } from "./NarrowTypes";

export class ConnectionTracker {
  private knownRelays: Map<number, ConnectionStatus> = new Map();
  private incomingCount = 0;
  private resolvedCount = 0;

  /**
   * Feed a new message into the tracker
   */
  handleMessage(msg: WorkerMessage) {
    const status = isConnectionStatus(msg);
    if (!status) return; // not a connection status, ignore

    const id = status.relayUrl()?.fnv1aHash()
    if(id && !this.knownRelays.has(id)) {
      this.incomingCount++;
     this.knownRelays.set(id, status);
    }
    if (this.isResolved(status)) {
      this.resolvedCount++;
    }
  }

  /**
   * Define what counts as a "resolved" connection.
   * Adjust based on your real ConnectionStatus enum/shape.
   */
  private isResolved(status: ConnectionStatus): boolean {
    return status.status()?.toString() === "EOSE";
  }

  /** Total connection attempts processed */
  get totalIncoming(): number {
    return this.incomingCount;
  }

  /** How many actually resolved */
  get totalResolved(): number {
    return this.resolvedCount;
  }

  /** Quick ratio (0...1) of resolved vs incoming */
  get resolutionRate(): number {
    return this.incomingCount === 0 ? 0 : this.resolvedCount / this.incomingCount;
  }
}
