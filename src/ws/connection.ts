import { ConnectionStatus, RelayConfig, RelayStats } from './types';

// Callback invoked for every incoming websocket text frame
export type MessageHandler = (
	url: string,
	subId: string | null, // present for EVENT/EOSE/CLOSED
	rawText: string
) => void;

export class RelayConnection {
	private wantReconnect = true;
	private url: string;
	private config: RelayConfig;
	private status: ConnectionStatus = ConnectionStatus.Closed;
	private ws: WebSocket | null = null;
	private reconnectTimer: number | null = null;
	private abortController: AbortController | null = null;

	private attempts: number = 0; // reconnection attempts after first failure
	private givenUp: boolean = false; // set when attempts reached cap
	private lastActivity: number = Date.now();
	private stats: RelayStats = {
		dropped: 0,
		sent: 0,
		received: 0,
		reconnects: 0, // mirrors attempts for visibility
		lastActivity: Date.now()
	};
	private readyWaiters: Array<(ok: boolean) => void> = [];
	public messageHandler: MessageHandler | null = null;

	constructor(url: string, config: Partial<RelayConfig> = {}) {
		this.url = url;
		this.config = {
			connectTimeoutMs: 5_000,
			writeTimeoutMs: 10_000,
			retry: {
				baseMs: 300,
				maxMs: 10_000,
				multiplier: 1.6,
				jitter: 0.1
			},
			maxReconnectAttempts: 2, // default: 2 retries
			idleTimeoutMs: 300_000,
			...config
		};
	}

	getUrl(): string {
		return this.url;
	}
	getStatus(): ConnectionStatus {
		return this.status;
	}
	getStats(): RelayStats {
		return { ...this.stats, reconnects: this.attempts, lastActivity: this.lastActivity };
	}
	getLastActivity(): number {
		return this.lastActivity;
	}
	hasGivenUp(): boolean {
		return this.givenUp;
	}

	setMessageHandler(handler: MessageHandler): void {
		this.messageHandler = handler;
	}

	// Fire-and-forget connect. It never throws. Use waitForReady() to await.
	connect(): void {
		if (this.givenUp) return;
		if (this.status === ConnectionStatus.Connecting || this.status === ConnectionStatus.Ready)
			return;

		// Clear pending reconnect
		if (this.reconnectTimer) {
			clearTimeout(this.reconnectTimer);
			this.reconnectTimer = null;
		}

		this.status = ConnectionStatus.Connecting;
		this.abortController = new AbortController();
		const signal = this.abortController.signal;

		try {
			this.ws = new WebSocket(this.url);
			this.ws.binaryType = 'arraybuffer';

			const cleanup = () => {
				if (!this.ws) return;
				this.ws.onopen = null as any;
				this.ws.onclose = null as any;
				this.ws.onerror = null as any;
				this.ws.onmessage = null as any;
			};

			const onOpen = () => {
				// Clear connect timeout
				clearTimeout(to);
				// Success: reset attempts and flags
				this.status = ConnectionStatus.Ready;
				this.attempts = 0;
				this.givenUp = false;
				this.lastActivity = Date.now();
				this.stats.lastActivity = this.lastActivity;
				this.resolveReady(true);
			};

			const onClose = (ev: CloseEvent) => {
				// Clear connect timeout
				clearTimeout(to);
				cleanup();
				this.status = ConnectionStatus.Closed;
				this.resolveReady(false);
				if (ev.code !== 1000) {
					this.scheduleReconnect();
				}
			};

			const onError = (_ev: Event) => {
				// Clear connect timeout
				clearTimeout(to);
				// Treat like close; don’t throw
				cleanup();
				this.status = ConnectionStatus.Closed;
				this.resolveReady(false);
				this.scheduleReconnect();
			};

			const onMessage = (event: MessageEvent) => {
				this.lastActivity = Date.now();
				this.stats.lastActivity = this.lastActivity;
				this.stats.received++;

				if (typeof event.data === 'string') {
					const rawText = event.data;
					const subId = this.extractSubId(rawText);
					if (this.messageHandler) {
						this.messageHandler(this.url, subId, rawText);
					}
				}
			};

			// Attach
			this.ws.onopen = onOpen;
			this.ws.onclose = onClose;
			this.ws.onerror = onError;
			this.ws.onmessage = onMessage;

			// Timeout and abort
			const to = setTimeout(() => {
				if (this.status === ConnectionStatus.Connecting) {
					cleanup();
					try {
						this.ws?.close();
					} catch {}
					this.status = ConnectionStatus.Closed;
					this.resolveReady(false);
					this.scheduleReconnect();
				}
			}, this.config.connectTimeoutMs);

			const onAbort = () => {
				clearTimeout(to);
				cleanup();
				try {
					this.ws?.close();
				} catch {}
				this.status = ConnectionStatus.Closed;
				this.resolveReady(false);
				if (this.wantReconnect) this.scheduleReconnect(); // guard here
			};

			signal.addEventListener('abort', onAbort);
		} catch {
			this.status = ConnectionStatus.Closed;
			this.resolveReady(false);
			this.scheduleReconnect();
		}
	}

	async sendMessage(frame: string): Promise<void> {
		if (this.status !== ConnectionStatus.Ready || !this.ws) {
			throw new Error('Connection not ready');
		}
		this.ws.send(frame);
		this.stats.sent++;
		this.lastActivity = Date.now();
		this.stats.lastActivity = this.lastActivity;
	}

	async waitForReady(timeoutMs: number = this.config.connectTimeoutMs ?? 5_000): Promise<void> {
		if (this.status === ConnectionStatus.Ready) return;

		return new Promise<void>((resolve, reject) => {
			const timer = setTimeout(() => {
				this.removeReadyResolver(resolver);
				reject(new Error('Timeout waiting for ready'));
			}, timeoutMs);

			const resolver = (ok: boolean) => {
				clearTimeout(timer);
				if (ok) resolve();
				else reject(new Error('Connection closed'));
			};
			this.readyWaiters.push(resolver);
		});
	}

	async close(): Promise<void> {
		this.wantReconnect = false;
		if (this.abortController) this.abortController.abort();
		if (this.reconnectTimer) {
			clearTimeout(this.reconnectTimer);
			this.reconnectTimer = null;
		}
		this.closeWebSocket();
		this.status = ConnectionStatus.Closed;
		this.resolveReady(false);
	}

	shouldCloseDueToInactivity(): boolean {
		return Date.now() - this.lastActivity > (this.config.idleTimeoutMs ?? 300_000);
	}

	// Internals

	private resolveReady(ok: boolean) {
		if (this.readyWaiters.length === 0) return;
		const waiters = this.readyWaiters.slice();
		this.readyWaiters.length = 0;
		for (const fn of waiters) {
			try {
				fn(ok);
			} catch {}
		}
	}

	private removeReadyResolver(fn: (ok: boolean) => void) {
		const idx = this.readyWaiters.indexOf(fn);
		if (idx >= 0) this.readyWaiters.splice(idx, 1);
	}

	private closeWebSocket() {
		if (this.ws) {
			try {
				this.ws.close(1000, 'Normal closure');
			} catch {}
			this.ws = null;
		}
	}

	private scheduleReconnect(): void {
		if (!this.wantReconnect) return;
		if (this.status !== ConnectionStatus.Closed || !this.config.retry?.baseMs) return;

		const cap = this.config.maxReconnectAttempts ?? 2;
		if (cap > 0 && this.attempts >= cap) {
			this.givenUp = true;
			return;
		}

		const base = this.config.retry.baseMs;
		const max = this.config.retry.maxMs ?? 10_000;
		const mult = this.config.retry.multiplier ?? 1.6;
		const jitter = this.config.retry.jitter ?? 0.1;

		// Calculate delay using current attempts
		const delay =
			Math.min(base * Math.pow(mult, this.attempts), max) *
			(1 + (Math.random() - 0.5) * jitter * 2);

		this.reconnectTimer = setTimeout(() => {
			// Increment attempts when actually trying again
			this.attempts++;
			this.stats.reconnects = this.attempts;
			this.connect();
		}, delay) as unknown as number;
	}

	private extractSubId(s: string): string | null {
		let i = 0;
		const n = s.length;

		// Skip leading whitespace
		while (i < n && s.charCodeAt(i) <= 32) i++;
		if (i >= n || s[i] !== '[') return null;
		i++;

		// Skip whitespace before first element
		while (i < n && s.charCodeAt(i) <= 32) i++;
		if (i >= n) return null;

		// Skip first element (kind or similar)
		if (s[i] === '"') {
			// Fast-skip a quoted string (no escape handling, consistent with existing code)
			i++;
			while (i < n && s[i] !== '"') i++;
			if (i >= n) return null;
			i++; // past closing quote
		} else {
			// Non-quoted first element; skip until comma or end of array
			while (i < n && s[i] !== ',' && s[i] !== ']') i++;
		}

		// Move to the comma after first element
		while (i < n && s[i] !== ',') i++;
		if (i >= n || s[i] !== ',') return null;
		i++; // skip comma

		// Skip whitespace before second element
		while (i < n && s.charCodeAt(i) <= 32) i++;
		if (i >= n) return null;

		// Extract second element only if it's a quoted string
		if (s[i] === '"') {
			i++;
			const start = i;
			while (i < n && s[i] !== '"') i++;
			if (i > n) return null;
			return s.slice(start, i);
		}
		return null;
	}
}
