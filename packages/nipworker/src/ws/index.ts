import * as flatbuffers from 'flatbuffers'; // Assume @flatbuffers/flatbuffers is available via bundler
import * as WorkerMessages from 'src/generated/nostr/fb'; // Generated from schemas/worker_messages.fbs
import { ByteRingBuffer, initializeRingHeader } from 'src/ws/ring-buffer';
import { MsgKind, RelayConfig } from 'src/ws/types';
import { ConnectionRegistry } from './registry';

// Message handler for connections: builds FlatBuffers WorkerLine and writes to output ring
function handleIncomingMessage(
	outputRing: ByteRingBuffer,
	url: string,
	kind: MsgKind,
	subId: string | null,
	rawText: string
): void {
	const encoder = new TextEncoder();
	const rawBytes = encoder.encode(rawText);

	// Build FlatBuffers WorkerLine
	const builder = new flatbuffers.Builder(1024);
	const relayOffset = WorkerMessages.RelayRef.createRelayRef(builder, builder.createString(url));
	const kindEnum = kind;

	let subIdOffset: flatbuffers.Offset | null = null;
	if (subId) {
		subIdOffset = builder.createString(subId);
	}

	const rawOffset = builder.createByteVector(rawBytes);

	WorkerMessages.WorkerLine.startWorkerLine(builder);
	WorkerMessages.WorkerLine.addRelay(builder, relayOffset);
	WorkerMessages.WorkerLine.addKind(builder, kindEnum);
	if (subIdOffset) {
		WorkerMessages.WorkerLine.addSubId(builder, subIdOffset);
	}
	WorkerMessages.WorkerLine.addRaw(builder, rawOffset);
	const lineOffset = WorkerMessages.WorkerLine.endWorkerLine(builder);

	builder.finish(lineOffset);
	const fbBytes = new Uint8Array(builder.asUint8Array());

	// Write to output ring
	outputRing.write(fbBytes);
}

class WSManager {
	private inRing: SharedArrayBuffer = new SharedArrayBuffer(1 * 1024 * 1024); // 1MB
	private outRing: SharedArrayBuffer = new SharedArrayBuffer(5 * 1024 * 1024); // 5MB
	private inputRing: ByteRingBuffer;
	private outputRing: ByteRingBuffer;
	private registry: ConnectionRegistry;

	// Adaptive backoff state
	private static readonly MIN_BACKOFF_MS = 10;
	private static readonly MAX_BACKOFF_MS = 1000;
	private static readonly BACKOFF_MULTIPLIER = 2;

	private inputLoopBackoffMs = WSManager.MIN_BACKOFF_MS;
	private inputLoopTimer: number | null = null;
	private decoder = new TextDecoder();

	constructor(config: RelayConfig) {
		initializeRingHeader(this.inRing);
		initializeRingHeader(this.outRing);

		// Create ring buffers
		this.inputRing = new ByteRingBuffer(this.inRing);
		this.outputRing = new ByteRingBuffer(this.outRing);

		// Create registry
		this.registry = new ConnectionRegistry(config || {});

		// Set up message handler for all future connections
		const globalMessageHandler = (
			url: string,
			kind: MsgKind,
			subId: string | null,
			rawText: string
		) => {
			handleIncomingMessage(this.outputRing, url, kind, subId, rawText);
		};

		// Override ensureConnection to set handler
		const originalEnsure = this.registry.ensureConnection.bind(this.registry);
		this.registry.ensureConnection = async (url: string) => {
			const conn = await originalEnsure(url);
			if (!conn.messageHandler) {
				conn.setMessageHandler(globalMessageHandler);
			}
			return conn;
		};

		// Start the input loop
		this.processInputLoop();

		console.log('WS Manager initialized');
	}

	// External API: reset the input-loop backoff to be aggressive immediately
	public resetInputLoopBackoff(): void {
		this.inputLoopBackoffMs = WSManager.MIN_BACKOFF_MS;
		if (this.inputLoopTimer !== null) {
			clearTimeout(this.inputLoopTimer);
			this.inputLoopTimer = null;
		}
		this.scheduleInputLoop();
	}

	// Scheduler helper
	private scheduleInputLoop(): void {
		const anyGlobal = globalThis as any;
		if (typeof anyGlobal.requestIdleCallback === 'function') {
			anyGlobal.requestIdleCallback(() => this.processInputLoop(), {
				timeout: this.inputLoopBackoffMs
			});
		} else {
			this.inputLoopTimer = setTimeout(
				() => this.processInputLoop(),
				this.inputLoopBackoffMs
			) as unknown as number;
		}
	}

	// Input processing loop: poll the input ring and dispatch envelopes with adaptive backoff
	private processInputLoop = (): void => {
		let processed = 0;

		while (true) {
			const record = this.inputRing.read();
			if (!record) break;

			const envelopeStr = this.decoder.decode(record);
			let envelope: any;
			try {
				envelope = JSON.parse(envelopeStr);
			} catch (e) {
				console.warn('Invalid envelope JSON:', e);
				continue;
			}

			if (!Array.isArray(envelope.relays) || !Array.isArray(envelope.frames)) {
				console.warn('Invalid envelope structure');
				continue;
			}

			// Fire-and-forget send to avoid blocking the loop
			this.registry.sendToRelays(envelope.relays, envelope.frames).catch(console.error);
			processed++;
		}

		// Adaptive backoff: reset when work was found; otherwise grow (capped)
		this.inputLoopBackoffMs =
			processed > 0
				? WSManager.MIN_BACKOFF_MS
				: Math.min(
						this.inputLoopBackoffMs * WSManager.BACKOFF_MULTIPLIER,
						WSManager.MAX_BACKOFF_MS
					);

		this.scheduleInputLoop();
	};

	getInRing(): SharedArrayBuffer {
		return this.inRing;
	}

	getOutRing(): SharedArrayBuffer {
		return this.outRing;
	}
}

export const wsManager = new WSManager({});
