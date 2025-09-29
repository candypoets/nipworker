import { RelayConfig, ConnectionStatus } from './types';
import { RelayConnection } from './connection';

export class ConnectionRegistry {
	private connections = new Map<string, RelayConnection>();
	private disabledRelays = new Set<string>();
	private nextAllowed = new Map<string, number>(); // cooldown timestamps per URL (ms)
	private subCounts = new Map<string, number>(); // active subscription count per URL
	private disconnectTimers = new Map<string, number>(); // delayed disconnect timers per URL
	private config: RelayConfig;

	// Cooldown & delayed close configuration
	private readonly cooldownMs = 60_000; // after failures
	private readonly closeDelayMs = 1_000; // after last CLOSE

	constructor(config: RelayConfig) {
		this.config = config;
	}

	private now(): number {
		return Date.now();
	}

	private detectKind(frame: string): 'REQ' | 'CLOSE' | 'OTHER' {
		const m = frame.match(/^\s*\[\s*"([^"]+)"/);
		if (!m) return 'OTHER';
		const k = m[1].toUpperCase();
		if (k === 'REQ') return 'REQ';
		if (k === 'CLOSE') return 'CLOSE';
		return 'OTHER';
	}

	private getCount(url: string): number {
		return this.subCounts.get(url) ?? 0;
	}

	private setCount(url: string, value: number): void {
		if (value <= 0) this.subCounts.set(url, 0);
		else this.subCounts.set(url, value);
	}

	private cancelPendingDisconnect(url: string): void {
		const t = this.disconnectTimers.get(url);
		if (typeof t === 'number') {
			clearTimeout(t);
			this.disconnectTimers.delete(url);
		}
	}

	private scheduleDisconnectIfIdle(url: string): void {
		if (this.disconnectTimers.has(url)) return;
		const id = setTimeout(async () => {
			this.disconnectTimers.delete(url);
			if (this.getCount(url) === 0) {
				await this.disconnect(url);
			}
		}, this.closeDelayMs) as unknown as number;
		this.disconnectTimers.set(url, id);
	}

	private bumpCount(url: string, delta: number): void {
		const prev = this.getCount(url);
		let next = prev + delta;
		if (next < 0) next = 0;
		this.setCount(url, next);

		if (delta > 0) {
			// Any new REQ cancels a pending idle close
			this.cancelPendingDisconnect(url);
		} else if (delta < 0 && next === 0) {
			console.log(`[registry] count for ${url} hit zero`);
			// When count hits zero, schedule delayed close
			this.scheduleDisconnectIfIdle(url);
		}
	}

	private shouldDisable(conn: RelayConnection): boolean {
		const cap = this.config.maxReconnectAttempts ?? 5;
		if (cap <= 0) return false;
		const stats = conn.getStats?.();
		if (!stats) return false;
		return stats.reconnects >= cap && conn.getStatus() !== ConnectionStatus.Ready;
	}

	private disable(url: string, reason?: string) {
		if (!this.disabledRelays.has(url)) {
			console.warn(`[registry] disabling relay ${url}${reason ? `: ${reason}` : ''}`);
		}
		this.disabledRelays.add(url);
		this.nextAllowed.set(url, this.now() + this.cooldownMs);
	}

	private isCoolingDown(url: string): boolean {
		const at = this.nextAllowed.get(url) ?? 0;
		return this.now() < at;
	}

	// Ensure a connection exists and is Ready (or throw)
	async ensureConnection(url: string): Promise<RelayConnection> {
		if (this.disabledRelays.has(url)) {
			throw new Error(`Relay disabled after repeated failures: ${url}`);
		}
		if (this.isCoolingDown(url)) {
			throw new Error(
				`Relay ${url} in cooldown until ${new Date(this.nextAllowed.get(url)!).toISOString()}`
			);
		}

		let conn = this.connections.get(url);
		if (!conn) {
			conn = new RelayConnection(url, this.config);
			this.connections.set(url, conn);
			conn.setMessageHandler(conn.messageHandler || null); // ensure property exists
			conn.connect(); // fire-and-forget
		}

		if (conn.getStatus() !== ConnectionStatus.Ready) {
			try {
				await conn.waitForReady(this.config.connectTimeoutMs ?? 30_000);
			} catch (e) {
				if (this.shouldDisable(conn)) {
					this.disable(url, 'max reconnect attempts reached');
				} else {
					// short cooldown to avoid hammering
					this.nextAllowed.set(url, this.now() + Math.min(this.cooldownMs, 10_000));
				}
				throw e;
			}
		}

		if (conn.getStatus() !== ConnectionStatus.Ready) {
			// still not ready; back off
			this.nextAllowed.set(url, this.now() + Math.min(this.cooldownMs, 10_000));
			throw new Error(`Relay ${url} not ready`);
		}

		return conn;
	}

	// Sends exactly one frame and updates REQ/CLOSE counters
	async sendFrame(url: string, frame: string): Promise<void> {
		if (this.disabledRelays.has(url) || this.isCoolingDown(url)) return;

		const kind = this.detectKind(frame);
		const conn = await this.ensureConnection(url);

		try {
			await conn.sendMessage(frame);
		} catch (e) {
			// Send failed; consider disabling/cooldown and disconnect
			if (this.shouldDisable(conn)) {
				this.disable(url, 'send failed and cap reached');
			} else {
				this.nextAllowed.set(url, this.now() + Math.min(this.cooldownMs, 10_000));
			}
			await this.disconnect(url);
			throw e;
		}

		if (kind === 'REQ') this.bumpCount(url, +1);
		else if (kind === 'CLOSE') this.bumpCount(url, -1);
	}

	async sendToRelays(relays: string[], frames: string[]): Promise<void> {
		for (const url of relays) {
			if (this.disabledRelays.has(url) || this.isCoolingDown(url)) continue;

			try {
				for (const frame of frames) {
					await this.sendFrame(url, frame);
				}
			} catch (error) {
				console.error(`[registry] failed to send to ${url}:`, error);
				// errors handled per sendFrame; continue to next relay
			}
		}
	}

	async disconnect(url: string): Promise<void> {
		console.log(`[registry] disconnecting from ${url}`);
		const connection = this.connections.get(url);
		if (connection) {
			await connection.close();
			this.connections.delete(url);
		}
		this.cancelPendingDisconnect(url);
		this.subCounts.delete(url);
	}

	async disconnectAll(): Promise<void> {
		for (const [url] of this.connections) {
			await this.disconnect(url);
		}
	}

	// Manual re-enable (e.g., via UI)
	enableRelay(url: string): void {
		this.disabledRelays.delete(url);
		this.nextAllowed.delete(url);
	}

	isRelayDisabled(url: string): boolean {
		return this.disabledRelays.has(url);
	}

	getActiveReqCount(url: string): number {
		return this.getCount(url);
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
