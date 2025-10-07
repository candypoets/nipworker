import { RelayConfig, ConnectionStatus } from './types';
import { RelayConnection } from './connection';

export class ConnectionRegistry {
	private connections = new Map<string, RelayConnection>();
	private disabledRelays = new Set<string>();
	private nextAllowed = new Map<string, number>(); // cooldown timestamps per URL (ms)
	private subCounts = new Map<string, number>(); // active subscription count per URL
	private disconnectTimers = new Map<string, number>(); // delayed disconnect timers
	private config: RelayConfig;

	private readonly cooldownMs = 60_000; // after failures
	private readonly closeDelayMs = 1_000; // after last CLOSE

	constructor(config: RelayConfig) {
		this.config = { maxReconnectAttempts: 2, ...config };
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
		this.subCounts.set(url, Math.max(0, value));
	}
	private cancelPendingDisconnect(url: string): void {
		const id = this.disconnectTimers.get(url);
		if (typeof id === 'number') {
			clearTimeout(id);
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
		const next = this.getCount(url) + delta;
		this.setCount(url, next);
		if (delta > 0) {
			this.cancelPendingDisconnect(url);
		} else if (delta < 0 && this.getCount(url) === 0) {
			this.scheduleDisconnectIfIdle(url);
		}
	}

	private giveUpOrCooldown(url: string, conn?: RelayConnection) {
		if (conn?.hasGivenUp()) {
			if (!this.disabledRelays.has(url)) {
				console.warn(`[registry] disabling relay ${url}: max attempts reached`);
			}
			this.disabledRelays.add(url);
			this.nextAllowed.set(url, this.now() + this.cooldownMs);
		} else {
			this.nextAllowed.set(url, this.now() + Math.min(this.cooldownMs, 10_000));
		}
	}

	private isCoolingDown(url: string): boolean {
		const at = this.nextAllowed.get(url) ?? 0;
		return this.now() < at;
	}

	async ensureConnection(url: string): Promise<RelayConnection> {
		if (this.disabledRelays.has(url)) {
			throw new Error(`Relay disabled: ${url}`);
		}
		if (this.isCoolingDown(url)) {
			throw new Error(
				`Relay ${url} cooling down until ${new Date(this.nextAllowed.get(url)!).toISOString()}`
			);
		}

		let conn = this.connections.get(url);
		if (!conn) {
			conn = new RelayConnection(url, this.config);
			this.connections.set(url, conn);
			conn.connect(); // fire-and-forget
		} else if (conn.getStatus() !== ConnectionStatus.Ready) {
			// nudge reconnect on existing connection
			conn.connect();
		}

		if (conn.getStatus() !== ConnectionStatus.Ready) {
			try {
				await conn.waitForReady(this.config.connectTimeoutMs ?? 5_000);
			} catch (e) {
				this.giveUpOrCooldown(url, conn);
				throw e;
			}
		}

		if (conn.getStatus() !== ConnectionStatus.Ready) {
			this.giveUpOrCooldown(url, conn);
			throw new Error(`Relay ${url} not ready`);
		}

		return conn;
	}

	async sendFrame(url: string, frame: string): Promise<void> {
		if (this.disabledRelays.has(url) || this.isCoolingDown(url)) return;

		const kind = this.detectKind(frame);
		const conn = await this.ensureConnection(url);

		try {
			await conn.sendMessage(frame);
		} catch (e) {
			this.giveUpOrCooldown(url, conn);
			await this.disconnect(url);
			throw e;
		}

		if (kind === 'REQ') this.bumpCount(url, +1);
		else if (kind === 'CLOSE') this.bumpCount(url, -1);
	}

	private async sendAllFramesToRelay(url: string, frames: string[]): Promise<void> {
		for (const frame of frames) {
			await this.sendFrame(url, frame);
		}
	}

	async sendToRelays(relays: string[], frames: string[]): Promise<void> {
		const tasks: Promise<void>[] = [];
		for (const url of relays) {
			if (this.disabledRelays.has(url) || this.isCoolingDown(url)) continue;

			tasks.push(
				this.sendAllFramesToRelay(url, frames).catch((error) => {
					console.error(`[registry] failed to send to ${url}:`, error);
				})
			);
		}

		await Promise.allSettled(tasks);
	}

	async disconnect(url: string): Promise<void> {
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
