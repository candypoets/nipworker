import { RelayConfig, ConnectionStatus } from './types';
import { RelayConnection } from './connection';
import { ByteRingBuffer } from './ring-buffer';

export class ConnectionRegistry {
  private connections = new Map<string, RelayConnection>();
  private outputRing?: ByteRingBuffer;

  constructor(outputRing: ByteRingBuffer, config: RelayConfig) {
    this.outputRing = outputRing;
    this.config = config;
  }

  async ensureConnection(url: string): Promise<RelayConnection> {
    if (!this.connections.has(url)) {
      const connection = new RelayConnection(url, this.config);
      this.connections.set(url, connection);
      await connection.connect();
    }
    const conn = this.connections.get(url)!;
    if (conn.getStatus() !== ConnectionStatus.Ready) {
      await conn.waitForReady();
    }
    return conn;
  }

  async sendToRelays(relays: string[], frames: string[]): Promise<void> {
    for (const url of relays) {
      try {
        const connection = await this.ensureConnection(url);
        for (const frame of frames) {
          await connection.sendMessage(frame);
        }
      } catch (error) {
        console.error(`Failed to send to ${url}:`, error);
        // Optionally disconnect on error
        this.disconnect(url);
      }
    }
  }

  sendFrame(url: string, frame: string): Promise<void> {
    return this.ensureConnection(url).then(conn => conn.sendMessage(frame));
  }

  async disconnect(url: string): Promise<void> {
    const connection = this.connections.get(url);
    if (connection) {
      await connection.close();
      this.connections.delete(url);
    }
  }

  async disconnectAll(): Promise<void> {
    for (const [url] of this.connections) {
      await this.disconnect(url);
    }
  }

  getConnectionStatus(url: string): ConnectionStatus | undefined {
    const connection = this.connections.get(url);
    return connection ? connection.getStatus() : undefined;
  }

  getAllStatuses(): Map<string, ConnectionStatus> {
    return new Map(
      Array.from(this.connections.entries()).map(([url, conn]) => [url, conn.getStatus()])
    );
  }
}
