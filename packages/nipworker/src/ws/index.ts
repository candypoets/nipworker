import * as flatbuffers from 'flatbuffers'; // Assume @flatbuffers/flatbuffers is available via bundler
import * as WorkerMessages from 'src/generated/nostr/fb'; // Generated from schemas/worker_messages.fbs
import { ByteRingBuffer, initializeRingHeader } from 'src/ws/ring-buffer';
import { MsgKind, RelayConfig } from 'src/ws/types';
import { ConnectionRegistry } from './registry';
import { NostrManager } from 'src/manager';

// Reuse encoder and cache URL bytes to avoid per-message URL encoding
const __encoder = new TextEncoder();
const __urlBytesCache = new Map<string, Uint8Array>();

function __getUrlBytes(url: string): Uint8Array {
	const cached = __urlBytesCache.get(url);
	if (cached) return cached;
	const bytes = __encoder.encode(url);
	__urlBytesCache.set(url, bytes);
	return bytes;
}

// Message handler for connections: builds FlatBuffers WorkerLine and writes to output ring
function handleIncomingMessage(outputRing: ByteRingBuffer, url: string, rawText: string): void {
	// Encode URL once (cached) and rawText once
	const urlBytes = __getUrlBytes(url);
	const rawBytes = __encoder.encode(rawText);

	// Envelope: [u16 urlLen][url][u32 rawLen][raw]
	const totalLen = 2 + urlBytes.length + 4 + rawBytes.length;
	const out = new Uint8Array(totalLen);
	const view = new DataView(out.buffer, out.byteOffset, out.byteLength);

	let o = 0;
	view.setUint16(o, urlBytes.length, false);
	o += 2; // big-endian
	out.set(urlBytes, o);
	o += urlBytes.length;

	view.setUint32(o, rawBytes.length, false);
	o += 4; // big-endian
	out.set(rawBytes, o);

	// Single write to the ring buffer
	outputRing.write(out);
}

export class NipWorker {
	private inRings: SharedArrayBuffer[] = [];
	private outRings: SharedArrayBuffer[] = [];
	private inputRings: ByteRingBuffer[] = [];
	private outputRings: ByteRingBuffer[] = [];
	private managers: NostrManager[] = [];
	private registry: ConnectionRegistry;

	private lastRingIndex = 0; // round-robin cursor for input ring

	// Adaptive backoff state
	private static readonly MIN_BACKOFF_MS = 10;
	private static readonly MAX_BACKOFF_MS = 1000;
	private static readonly BACKOFF_MULTIPLIER = 2;

	private inputLoopBackoffMs = NipWorker.MIN_BACKOFF_MS;
	private inputLoopTimer: number | null = null;
	private decoder = new TextDecoder();

	private hashSubId(sub_id: string): number {
		let hash = 0;
		for (let i = 0; i < sub_id.length; i++) {
			hash = (hash << 5) - hash + sub_id.charCodeAt(i);
		}
		return Math.abs(hash) % this.managers.length;
	}

	// Somewhere in the class:
	private subIdToRing = new Map<string, ByteRingBuffer>();

	private getOutRingForSubId(subId: string): ByteRingBuffer {
		let ring = this.subIdToRing.get(subId);
		if (ring) return ring;

		// Fallback: compute once, then cache
		const idx = this.hashSubId(subId);
		ring = this.outputRings[idx] as ByteRingBuffer;
		this.subIdToRing.set(subId, ring);
		return ring;
	}

	public createShortId(input: string): string {
		if (input.length < 64) return input;
		let hash = 0;
		for (let i = 0; i < input.length; i++) {
			const char = input.charCodeAt(i);
			hash = (hash << 5) - hash + char;
			hash = hash & hash;
		}
		const shortId = Math.abs(hash).toString(36);
		return shortId.substring(0, 63);
	}

	constructor(config: RelayConfig, scale = 3) {
		const cores = navigator.hardwareConcurrency ?? 1;
		console.log('cores', cores);
		for (let i = 0; i < scale; i++) {
			this.inRings.push(new SharedArrayBuffer(1 * 1024 * 1024)); // 1MB
			this.outRings.push(new SharedArrayBuffer(5 * 1024 * 1024)); // 5MB
			initializeRingHeader(this.inRings[i] as SharedArrayBuffer);
			initializeRingHeader(this.outRings[i] as SharedArrayBuffer);
			this.managers.push(
				new NostrManager({
					bufferKey: i.toString(),
					maxBufferSize: 2_000_000,
					inRing: this.inRings[i] as SharedArrayBuffer,
					outRing: this.outRings[i] as SharedArrayBuffer
				})
			);
			this.inputRings.push(new ByteRingBuffer(this.inRings[i] as SharedArrayBuffer));
			this.outputRings.push(new ByteRingBuffer(this.outRings[i] as SharedArrayBuffer));
		}

		// Create registry
		this.registry = new ConnectionRegistry(config || {});

		// Set up message handler for all future connections
		const globalMessageHandler = (url: string, subId: string | null, rawText: string) => {
			const outRing = this.getOutRingForSubId(subId as string);
			handleIncomingMessage(outRing, url, rawText);
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

	public cleanup(): void {
		for (const manager of this.managers) {
			manager.cleanup();
		}
	}

	public setSigner(name: string, secretKeyHex: string): void {
		for (const manager of this.managers) {
			manager.setSigner(name, secretKeyHex);
		}
	}

	public getManager(subId: string): NostrManager {
		subId = this.createShortId(subId);
		const hash = this.hashSubId(subId || '');
		return this.managers[hash] as NostrManager;
	}

	// External API: reset the input-loop backoff to be aggressive immediately
	public resetInputLoopBackoff(): void {
		this.inputLoopBackoffMs = NipWorker.MIN_BACKOFF_MS;
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
	// Input processing loop: poll all input rings and dispatch envelopes with adaptive backoff
	private processInputLoop = (): void => {
		let processed = 0;

		const ringCount = this.inputRings.length;
		if (ringCount === 0) {
			this.scheduleInputLoop();
			return;
		}

		// Keep looping as long as at least one ring produced a record in the last pass
		let madeProgress: boolean;
		do {
			madeProgress = false;

			for (let i = 0; i < ringCount; i++) {
				const idx = (this.lastRingIndex + i) % ringCount;
				const ring = this.inputRings[idx] as ByteRingBuffer;

				const record = ring.read();
				if (!record) {
					continue;
				}

				madeProgress = true;
				processed++;

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

				// Fire-and-forget to avoid blocking the loop
				this.registry.sendToRelays(envelope.relays, envelope.frames).catch(console.error);

				// Advance the round-robin starting point to the next ring after the one that yielded data
				this.lastRingIndex = (idx + 1) % ringCount;
			}
		} while (madeProgress);

		// Adaptive backoff: reset when work was found; otherwise grow (capped)
		this.inputLoopBackoffMs =
			processed > 0
				? NipWorker.MIN_BACKOFF_MS
				: Math.min(
						this.inputLoopBackoffMs * NipWorker.BACKOFF_MULTIPLIER,
						NipWorker.MAX_BACKOFF_MS
					);

		this.scheduleInputLoop();
	};
}

export const nipWorker = new NipWorker({});
