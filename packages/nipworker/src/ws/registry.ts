import { RelayConfig, ConnectionStatus } from './types';
import { RelayConnection } from './connection';

export class ConnectionRegistry {
	private connections = new Map<string, RelayConnection>();
	private disabledRelays = new Set<string>();
	private nextAllowed = new Map<string, number>(); // cooldown timestamps per URL (ms)
	private subCounts = new Map<string, number>(); // active subscription count per URL
	private config: RelayConfig;

	private readonly cooldownMs = 60_000; // after failures

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

	// Replace bumpCount with a pure counter that returns the new value.
	private bumpCount(url: string, delta: number): number {
		const next = Math.max(0, this.getCount(url) + delta);
		this.subCounts.set(url, next);
		return next;
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

	// Update sendFrame to disconnect immediately when count becomes 0 after a CLOSE.
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

		// Track the sub count for visibility and lifecycle decisions
		if (kind === 'REQ') {
			this.bumpCount(url, +1);
		} else if (kind === 'CLOSE') {
			const newCount = this.bumpCount(url, -1);
			if (newCount === 0) {
				console.log('disconnecting', url);
				// Immediately disconnect when no more active REQ for this relay
				await this.disconnect(url);
			}
		}
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
		// No pending-disconnects to cancel anymore
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
