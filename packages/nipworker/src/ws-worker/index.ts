// nipworker/packages/nipworker/src/ws-worker/index.ts

/// <reference lib="webworker" />

import * as flatbuffers from 'flatbuffers'; // Assume @flatbuffers/flatbuffers is available via bundler
import * as WorkerMessages from 'src/generated/nostr/fb'; // Generated from schemas/worker_messages.fbs
import { ByteRingBuffer } from 'src/ws-worker/ring-buffer';
import { MsgKind } from 'src/ws-worker/types';
import { ConnectionRegistry } from './registry';

// Message handler for connections: builds FlatBuffers WorkerLine and writes to output ring
function handleIncomingMessage(outputRing: ByteRingBuffer, url: string, kind: MsgKind, subId: string | null, rawText: string): void {
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

// Main WebWorker entrypoint
declare const self: DedicatedWorkerGlobalScope;

self.onmessage = async (event: MessageEvent) => {
  if (event.data.type === 'init') {
    const { inRing, outRing, config }: { inRing: SharedArrayBuffer; outRing: SharedArrayBuffer; config?: RelayConfig } = event.data;

    // Create ring buffers
    const inputRing = new ByteRingBuffer(inRing);
    const outputRing = new ByteRingBuffer(outRing);

    // Create registry
    const registry = new ConnectionRegistry(outputRing, config || {});

    // Set up message handler for all future connections
    const globalMessageHandler = (url: string, kind: MsgKind, subId: string | null, rawText: string) => {
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
        console.log('record found', envelope.relays, envelope.frames);
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

    // Optional: respond to init
    self.postMessage({ type: 'initialized' });

    console.log('WS Worker initialized');
  } else if (event.data.type === 'shutdown') {
    // Optional shutdown handling
    self.close();
  }
};

// Export for type checking if needed, but worker doesn't need it
export {};

// Idempotent header initializer for rings created on the TS side.
// If capacity (u32 at offset 0) is 0, we set it to (byteLength - 32)
// and zero head, tail, and seq. Reserved bytes are cleared as well.
export function initializeRingHeader(buffer: SharedArrayBuffer): void {
  const HEADER = 32;
  const view = new DataView(buffer);
  const total = buffer.byteLength;

  if (total < HEADER) {
    throw new Error(`Ring buffer too small: ${total} bytes`);
  }

  const cap = view.getUint32(0, true);
  if (cap !== 0) {
    // Already initialized; nothing to do.
    return;
  }

  const capacity = total - HEADER;
  if (capacity <= 0) {
    throw new Error(`Invalid ring capacity computed from total=${total}`);
  }

  // Initialize header: capacity, head=0, tail=0, seq=0, reserved=0
  view.setUint32(0, capacity, true); // capacity
  view.setUint32(4, 0, true);        // head
  view.setUint32(8, 0, true);        // tail
  view.setUint32(12, 0, true);       // seq
  // Zero reserved [16..32)
  for (let off = 16; off < 32; off += 4) {
    view.setUint32(off, 0, true);
  }
}
