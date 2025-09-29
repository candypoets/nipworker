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
	private inRing: SharedArrayBuffer = new SharedArrayBuffer(1 * 1024 * 1024); // 1MB;
	private outRing: SharedArrayBuffer = new SharedArrayBuffer(5 * 1024 * 1024); // 5MB;

	constructor(config: RelayConfig) {
		initializeRingHeader(this.inRing);
		initializeRingHeader(this.outRing);

		// Create ring buffers
		const inputRing = new ByteRingBuffer(this.inRing);
		const outputRing = new ByteRingBuffer(this.outRing);

		// Create registry
		const registry = new ConnectionRegistry(config || {});

		// Set up message handler for all future connections
		const globalMessageHandler = (
			url: string,
			kind: MsgKind,
			subId: string | null,
			rawText: string
		) => {
			handleIncomingMessage(outputRing, url, kind, subId, rawText);
		};

		// Override ensureConnection to set handler
		const originalEnsure = registry.ensureConnection.bind(registry);
		registry.ensureConnection = async (url: string) => {
			const conn = await originalEnsure(url);
			if (!conn.messageHandler) {
				conn.setMessageHandler(globalMessageHandler);
			}
			return conn;
		};

		// Input processing loop: poll the input ring and dispatch envelopes
		const processInputLoop = () => {
			// console.log("processInputLoop")
			while (true) {
				const record = inputRing.read();
				if (!record) break;
				// Decode payload as UTF-8
				const decoder = new TextDecoder();
				const envelopeStr = decoder.decode(record);
				let envelope;
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
				// Send frames to relays
				registry.sendToRelays(envelope.relays, envelope.frames).catch(console.error);
			}

			// Continue polling; use requestIdleCallback for efficiency if available, else setTimeout
			if (typeof (self as any).requestIdleCallback === 'function') {
				(self as any).requestIdleCallback(processInputLoop, { timeout: 1 });
			} else {
				setTimeout(processInputLoop, 1);
			}
		};

		// Start the input loop
		processInputLoop();

		console.log('WS Worker initialized');
	}

	getInRing(): SharedArrayBuffer {
		return this.inRing;
	}

	getOutRing(): SharedArrayBuffer {
		return this.outRing;
	}
}

export const wsManager = new WSManager({});
