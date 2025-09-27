import { ConnectionStatus, RelayConfig, RelayStats } from './types';

// Handler for incoming messages: (url, kind, subId, rawText) => void
type MessageHandler = (url: string, kind: number, subId: string | null, rawText: string) => void;

export class RelayConnection {
  private url: string;
  private config: RelayConfig;
  private status: ConnectionStatus = ConnectionStatus.Closed;
  private ws: WebSocket | null = null;
  private reconnectTimer: number | null = null; // In browser, use setTimeout ID
  private abortController: AbortController | null = null;
  private lastActivity: number = Date.now();
  private stats: RelayStats = {
    dropped: 0,
    sent: 0,
    received: 0,
    reconnects: 0,
    lastActivity: Date.now(),
  };
  private messageHandler: MessageHandler | null = null;

  constructor(url: string, config: Partial<RelayConfig> = {}) {
    this.url = url;
    this.config = {
      connectTimeoutMs: 10000,
      retry: {
        baseMs: 300,
        maxMs: 10000,
        multiplier: 1.6,
        jitter: 0.1,
      },
      idleTimeoutMs: 300000, // 5 min
      ...config,
    };
  }

  getUrl(): string {
    return this.url;
  }

  getStatus(): ConnectionStatus {
    return this.status;
  }

  getStats(): RelayStats {
    return { ...this.stats };
  }

  getLastActivity(): number {
    return this.lastActivity;
  }

  setMessageHandler(handler: MessageHandler): void {
    this.messageHandler = handler;
  }

  async connect(): Promise<void> {
    if (this.status === ConnectionStatus.Connecting || this.status === ConnectionStatus.Ready) {
      return;
    }

    this.status = ConnectionStatus.Connecting;
    this.abortController = new AbortController();
    const signal = this.abortController.signal;

    try {
      // Clear any existing reconnect timer
      if (this.reconnectTimer) {
        clearTimeout(this.reconnectTimer as any);
        this.reconnectTimer = null;
      }

      this.ws = new WebSocket(this.url);
      this.ws.binaryType = 'arraybuffer'; // For potential binary, but mostly text

      // Timeout for connect
      const timeoutId = setTimeout(() => {
        if (this.status === ConnectionStatus.Connecting) {
          this.closeWebSocket();
          this.scheduleReconnect();
        }
      }, this.config.connectTimeoutMs);

      await Promise.race([
        new Promise((resolve, reject) => {
          this.ws!.onopen = () => {
            clearTimeout(timeoutId);
            this.status = ConnectionStatus.Ready;
            this.lastActivity = Date.now();
            this.lastActivity = Date.now();
            resolve(undefined);
          };

          this.ws!.onclose = (event) => {
            clearTimeout(timeoutId);
            this.status = ConnectionStatus.Closed;
            if (event.code !== 1000) { // Not normal closure
              this.scheduleReconnect();
            }
            this.onClose();
          };

          this.ws!.onerror = (event) => {
            clearTimeout(timeoutId);
            reject(new Error(`WebSocket error: ${event}`));
          };

          this.ws!.onmessage = (event) => {
            this.handleMessage(event);
          };
        }),
        new Promise((_, reject) => signal.addEventListener('abort', () => reject(new DOMException('Aborted')))),
      ]);
    } catch (error) {
      this.status = ConnectionStatus.Closed;
      this.scheduleReconnect();
      throw error;
    }
  }

  private handleMessage(event: MessageEvent): void {
    this.lastActivity = Date.now();
    this.stats.received++;

    if (typeof event.data === 'string') {
      const rawText = event.data;
      const { kind, subId } = this.shallowScan(rawText);
      if (this.messageHandler) {
        this.messageHandler(this.url, kind, subId, rawText);
      }
    } else if (event.data instanceof ArrayBuffer) {
      // Handle binary if needed, but for Nostr, usually text. Log or forward as raw.
      console.warn('Received binary message from relay:', event.data);
      // Could forward as Uint8Array if needed, but skip for now.
    }
  }

  private shallowScan(rawText: string): { kind: number; subId: string | null } {
    try {
      // Minimal scan: find first [, then first "string" for kind, second for sub_id if applicable
      // This is a simple regex-based peek, no full parse
      const kindMatch = rawText.match(/"([^"]+)"/);
      if (!kindMatch) {
        return { kind: 0, subId: null };
      }

      const kindStr = kindMatch[1].toUpperCase();
      let kind: number = 0; // Unknown

      switch (kindStr) {
        case 'EVENT':
          kind = 1;
          break;
        case 'EOSE':
          kind = 2;
          break;
        case 'OK':
          kind = 3;
          break;
        case 'CLOSED':
          kind = 4;
          break;
        case 'NOTICE':
          kind = 5;
          break;
        case 'AUTH':
          kind = 6;
          break;
      }

      let subId: string | null = null;
      if (kind === 1 || kind === 2 || kind === 4) {
        // Find second quoted string using exec to advance position
        const regex = /"([^"]+)"/g;
        let match = regex.exec(rawText); // First match for kind (already handled)
        if (match) {
          match = regex.exec(rawText); // Second match for sub_id
          if (match) {
            subId = match[1];
          }
        }
      }

      return { kind, subId };
    } catch {
      return { kind: 0, subId: null };
    }
  }

  async sendMessage(frame: string): Promise<void> {
    if (this.status !== ConnectionStatus.Ready || !this.ws) {
      throw new Error('Connection not ready');
    }

    try {
      this.ws.send(frame);
      this.stats.sent++;
      this.lastActivity = Date.now();
    } catch (error) {
      this.stats.sent++; // Still count attempt
      throw error;
    }
  }

  private scheduleReconnect(): void {
    if (this.status === ConnectionStatus.Closed && this.config.retry?.baseMs) {
      const baseMs = this.config.retry.baseMs;
      const maxMs = this.config.retry.maxMs || 10000;
      const multiplier = this.config.retry.multiplier || 1.6;
      const jitter = this.config.retry.jitter || 0.1;
      const delay = Math.min(
        baseMs * Math.pow(multiplier, this.stats.reconnects),
        maxMs
      ) * (1 + (Math.random() - 0.5) * jitter * 2);

      this.reconnectTimer = setTimeout(() => {
        this.stats.reconnects++;
        this.connect();
      }, delay);
    }
  }

  async close(): Promise<void> {
    if (this.abortController) {
      this.abortController.abort();
    }
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer as any);
      this.reconnectTimer = null;
    }
    this.closeWebSocket();
    this.status = ConnectionStatus.Closed;
  }

  private closeWebSocket(): void {
    if (this.ws) {
      this.ws.close(1000, 'Normal closure');
      this.ws = null;
    }
  }

  private onClose(): void {
    // Optional: check inactivity, but for now, just log
    if (this.messageHandler) {
      // Could notify, but keep minimal
    }
  }

  // Check if should close due to inactivity
  shouldCloseDueToInactivity(): boolean {
    return Date.now() - this.lastActivity > this.config.idleTimeoutMs;
  }

  // Wait for ready with timeout
  async waitForReady(timeoutMs: number = 30000): Promise<void> {
    if (this.status === ConnectionStatus.Ready) {
      return;
    }

    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('Timeout waiting for ready')), timeoutMs);
      const checkInterval = setInterval(() => {
        if (this.status === ConnectionStatus.Ready) {
          clearTimeout(timer);
          clearInterval(checkInterval);
          resolve();
        } else if (this.status === ConnectionStatus.Closed) {
          clearTimeout(timer);
          clearInterval(checkInterval);
          reject(new Error('Connection closed'));
        }
      }, 100);
    });
  }
}
