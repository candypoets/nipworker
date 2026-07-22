// Shared types for the head-to-head comparison harness.

export interface RunResult {
	/** Events actually delivered to the consumer callback (after the lib's own dedup, if any). */
	received: number;
	/** Events seen on the wire / passed through the lib's ingest path before any dedup. */
	rawCount: number;
	notes: string[];
}

export interface ContenderRunner {
	name: string;
	/**
	 * What the contender does per event with the settings used here, and which
	 * of its defaults were overridden (e.g. signature verification must be
	 * disabled because the mock relay serves synthetic unsigned events).
	 */
	perEventWork: string[];
	/** Boot libraries / connections once per page. */
	setup(relay: string): Promise<void>;
	/**
	 * Subscribe to kinds:[1] with limit=n on `relay` and resolve once n events
	 * were delivered or the relay sent EOSE (plus a short drain). `onEvent` is
	 * called for every event delivered to the consumer.
	 */
	run(relay: string, n: number, subId: string, onEvent: () => void): Promise<RunResult>;
	teardown(): Promise<void>;
}
