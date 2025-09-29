import { ConnectionStatus, RelayConfig, RelayStats } from './types';

// Callback invoked for every incoming websocket text frame
export type MessageHandler = (
	url: string,
	kind: number, // 1=EVENT, 2=EOSE, 3=OK, 4=CLOSED, 5=NOTICE, 6=AUTH, 0=UNKNOWN
	subId: string | null, // present for EVENT/EOSE/CLOSED
	rawText: string
) => void;

export class RelayConnection {
	private url: string;
	private config: RelayConfig;
	private status: ConnectionStatus = ConnectionStatus.Closed;
	private ws: WebSocket | null = null;
	private reconnectTimer: number | null = null;
	private abortController: AbortController | null = null;
	private lastActivity: number = Date.now();
	private stats: RelayStats = {
		dropped: 0,
		sent: 0,
		received: 0,
		reconnects: 0,
		lastActivity: Date.now()
	};
	private readyWaiters: Array<(ok: boolean) => void> = [];
	public messageHandler: MessageHandler | null = null;

	constructor(url: string, config: Partial<RelayConfig> = {}) {
		this.url = url;
		this.config = {
			connectTimeoutMs: 10_000,
			writeTimeoutMs: 10_000,
			retry: {
				baseMs: 300,
				maxMs: 10_000,
				multiplier: 1.6,
				jitter: 0.1
			},
			maxReconnectAttempts: 5, // 0 => unlimited
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
		return { ...this.stats };
	}

	getLastActivity(): number {
		return this.lastActivity;
	}

	setMessageHandler(handler: MessageHandler): void {
		this.messageHandler = handler;
	}

	// Fire-and-forget: starts/attempts a connection. It never throws.
	// Use waitForReady() to await readiness.
	connect(): void {
		if (this.status === ConnectionStatus.Connecting || this.status === ConnectionStatus.Ready) {
			return;
		}

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
				// eslint-disable-next-line @typescript-eslint/no-empty-function
				this.ws.onopen = null as any;
				// eslint-disable-next-line @typescript-eslint/no-empty-function
				this.ws.onclose = null as any;
				// eslint-disable-next-line @typescript-eslint/no-empty-function
				this.ws.onerror = null as any;
				// eslint-disable-next-line @typescript-eslint/no-empty-function
				this.ws.onmessage = null as any;
			};

			const onOpen = () => {
				if (this.status !== ConnectionStatus.Connecting && this.status !== ConnectionStatus.Ready) {
					return;
				}
				this.status = ConnectionStatus.Ready;
				this.lastActivity = Date.now();
				this.stats.lastActivity = this.lastActivity;
				// Reset reconnect attempts
				this.stats.reconnects = 0;
				this.resolveReady(true);
			};

			const onClose = (ev: CloseEvent) => {
				cleanup();
				this.status = ConnectionStatus.Closed;
				this.resolveReady(false);
				// Schedule reconnect if abnormal close and retry policy allows
				if (ev.code !== 1000) {
					this.scheduleReconnect();
				}
			};

			const onError = (_ev: Event) => {
				// Treat as a close for retry purposes, but avoid throwing.
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
					const { kind, subId } = this.shallowScan(rawText);
					if (this.messageHandler) {
						this.messageHandler(this.url, kind, subId, rawText);
					}
				}
			};

			// Attach handlers
			this.ws.onopen = onOpen;
			this.ws.onclose = onClose;
			this.ws.onerror = onError;
			this.ws.onmessage = onMessage;

			// Connect timeout and abort
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

			signal.addEventListener('abort', () => {
				clearTimeout(to);
				cleanup();
				try {
					this.ws?.close();
				} catch {}
				this.status = ConnectionStatus.Closed;
				this.resolveReady(false);
				this.scheduleReconnect();
			});
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
		// Optional write timeout; in browsers send() is sync, but keep parity with config
		this.ws.send(frame);
		this.stats.sent++;
		this.lastActivity = Date.now();
		this.stats.lastActivity = this.lastActivity;
	}

	async waitForReady(timeoutMs: number = 30_000): Promise<void> {
		if (this.status === ConnectionStatus.Ready) return;

		return new Promise<void>((resolve, reject) => {
			const timer = setTimeout(() => {
				// Timeout waiting
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
		if (this.abortController) {
			this.abortController.abort();
		}
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

	// Internal

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
		if (this.status !== ConnectionStatus.Closed || !this.config.retry?.baseMs) return;

		const cap = this.config.maxReconnectAttempts ?? 5;
		if (cap > 0 && this.stats.reconnects >= cap) {
			// Give up
			return;
		}

		const base = this.config.retry.baseMs;
		const max = this.config.retry.maxMs ?? 10_000;
		const mult = this.config.retry.multiplier ?? 1.6;
		const jitter = this.config.retry.jitter ?? 0.1;

		const delay =
			Math.min(base * Math.pow(mult, this.stats.reconnects), max) *
			(1 + (Math.random() - 0.5) * jitter * 2);

		this.reconnectTimer = setTimeout(() => {
			this.stats.reconnects++;
			this.connect(); // fire-and-forget
		}, delay) as unknown as number;
	}

	// Efficient shallow scanner: extracts kind and optional subId for EVENT/EOSE/CLOSED
	private shallowScan(s: string): { kind: number; subId: string | null } {
		// Expect ["KIND", ...]
		let i = 0;
		const n = s.length;

		// skip ws
		while (i < n && s.charCodeAt(i) <= 32) i++;
		if (i >= n || s[i] !== '[') return { kind: 0, subId: null };
		i++;
		while (i < n && s.charCodeAt(i) <= 32) i++;
		if (i >= n || s[i] !== '"') return { kind: 0, subId: null };
		i++;
		let start = i;
		while (i < n && s[i] !== '"') i++;
		const kindStr = s.slice(start, i).toUpperCase();
		i++; // skip closing quote

		let kind = 0;
		if (kindStr === 'EVENT') kind = 1;
		else if (kindStr === 'EOSE') kind = 2;
		else if (kindStr === 'OK') kind = 3;
		else if (kindStr === 'CLOSED') kind = 4;
		else if (kindStr === 'NOTICE') kind = 5;
		else if (kindStr === 'AUTH') kind = 6;

		if (kind === 1 || kind === 2 || kind === 4) {
			// seek comma, then possible subId (string)
			while (i < n && s[i] !== ',') i++;
			if (i < n && s[i] === ',') i++;
			while (i < n && s.charCodeAt(i) <= 32) i++;
			if (i < n && s[i] === '"') {
				i++;
				start = i;
				while (i < n && s[i] !== '"') i++;
				const subId = s.slice(start, i);
				return { kind, subId };
			}
		}
		return { kind, subId: null };
	}
}
