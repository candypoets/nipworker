// Shared types for the multi-relay comparison harness.
//
// Scenario: N mock relays (tests/bench/mock-relay.mjs --seed-by filter
// --unique-fraction 0.2) all serve the same 2,000-event kinds:[1] set, with
// 80% of the events byte-identical across relays and a 20% deterministic
// per-relay unique tail. Duplicates arrive across relays exactly like real
// Nostr; the driver counts what each library actually delivers to the app.

export interface MultiRunResult {
	/** Consumer callbacks fired (what the app saw, after the lib's own dedup if any). */
	totalDelivered: number;
	/** ms from subscribe start until all relay sockets reported open (-1 if not reached). */
	connectAllMs: number;
	/** ms from subscribe start until EOSE observed from every relay (-1 if not reached). */
	allEoseMs: number;
	/** ms from subscribe start until the first delivered event (-1 if none). */
	firstEventMs: number;
	relaysConnected: number;
	relaysEosed: number;
	notes: string[];
}

export interface MultiRelayRunner {
	name: string;
	/** What the contender does per event with the settings used here, and which
	 *  of its defaults were overridden (mock relay serves unsigned events). */
	perEventWork: string[];
	/** Boot libraries once per page. Must NOT pre-connect the relays: connection
	 *  setup is part of the measured window. */
	setup(relays: string[]): Promise<void>;
	/**
	 * Subscribe to kinds:[1] with limit=n on every relay in `relays`.
	 * `onEvent` is called for every event delivered to the consumer, with the
	 * event id (the driver does the unique/dup accounting). Resolve once EOSE
	 * was seen from all relays (plus a short drain) or after `timeoutMs`,
	 * whichever comes first — partial results on timeout, with a note.
	 */
	run(
		relays: string[],
		n: number,
		subId: string,
		onEvent: (id: string) => void,
		timeoutMs: number
	): Promise<MultiRunResult>;
	teardown(): Promise<void>;
}

/** Small state tracker shared by the runners: per-relay connect/EOSE sets. */
export class RelayTracker {
	readonly t0 = performance.now();
	private connected = new Set<string>();
	private eosed = new Set<string>();
	connectAllMs = -1;
	allEoseMs = -1;
	firstEventMs = -1;
	totalDelivered = 0;

	constructor(private relayCount: number) {}

	markOpen(key: string): void {
		if (this.connected.has(key)) return;
		this.connected.add(key);
		if (this.connected.size >= this.relayCount && this.connectAllMs < 0) {
			this.connectAllMs = performance.now() - this.t0;
		}
	}

	markEose(key: string): void {
		if (this.eosed.has(key)) return;
		this.eosed.add(key);
		if (this.eosed.size >= this.relayCount && this.allEoseMs < 0) {
			this.allEoseMs = performance.now() - this.t0;
		}
	}

	markEvent(): void {
		this.totalDelivered++;
		if (this.firstEventMs < 0) this.firstEventMs = performance.now() - this.t0;
	}

	get relaysConnected(): number {
		return this.connected.size;
	}

	get relaysEosed(): number {
		return this.eosed.size;
	}

	result(notes: string[]): MultiRunResult {
		return {
			totalDelivered: this.totalDelivered,
			connectAllMs: round(this.connectAllMs),
			allEoseMs: round(this.allEoseMs),
			firstEventMs: round(this.firstEventMs),
			relaysConnected: this.relaysConnected,
			relaysEosed: this.relaysEosed,
			notes
		};
	}
}

export function round(v: number): number {
	return v < 0 ? v : Math.round(v * 100) / 100;
}
