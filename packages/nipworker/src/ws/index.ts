/* Worker runtime (dedicated Web Worker, module) */
import { ByteRingBuffer } from 'src/ws/ring-buffer';
import { ConnectionRegistry } from 'src/ws/registry';

const __encoder = new TextEncoder();
const __urlBytesCache = new Map<string, Uint8Array>();
function __getUrlBytes(url: string): Uint8Array {
	const cached = __urlBytesCache.get(url);
	if (cached) return cached;
	const bytes = __encoder.encode(url);
	__urlBytesCache.set(url, bytes);
	return bytes;
}

function writeEnvelope(outputRing: ByteRingBuffer, url: string, rawText: string): void {
	const urlBytes = __getUrlBytes(url);
	const rawBytes = __encoder.encode(rawText);
	const totalLen = 2 + urlBytes.length + 4 + rawBytes.length;
	const out = new Uint8Array(totalLen);
	const view = new DataView(out.buffer, out.byteOffset, out.byteLength);
	let o = 0;
	view.setUint16(o, urlBytes.length, false);
	o += 2;
	out.set(urlBytes, o);
	o += urlBytes.length;
	view.setUint32(o, rawBytes.length, false);
	o += 4;
	out.set(rawBytes, o);
	outputRing.write(out);
}

function extractSubIdFast(s: string): string | null {
	let i = 0,
		n = s.length;
	while (i < n && s.charCodeAt(i) <= 32) i++;
	if (i >= n || s[i] !== '[') return null;
	i++;
	while (i < n && s.charCodeAt(i) <= 32) i++;
	if (i >= n) return null;
	if (s[i] === '"') {
		i++;
		while (i < n && s[i] !== '"') i++;
		if (i >= n) return null;
		i++;
	} else {
		while (i < n && s[i] !== ',' && s[i] !== ']') i++;
	}
	while (i < n && s[i] !== ',') i++;
	if (i >= n || s[i] !== ',') return null;
	i++;
	while (i < n && s.charCodeAt(i) <= 32) i++;
	if (i >= n) return null;
	if (s[i] === '"') {
		i++;
		const start = i;
		while (i < n && s[i] !== '"') i++;
		if (i > n) return null;
		return s.slice(start, i);
	}
	return null;
}

type InitMsg = {
	type: 'init';
	payload: {
		inRings: SharedArrayBuffer[];
		outRings: SharedArrayBuffer[];
		relayConfig: any;
	};
};

let inputRings: ByteRingBuffer[] = [];
let outputRings: ByteRingBuffer[] = [];
let registry: ConnectionRegistry | null = null;
const subIdToRing = new Map<string, ByteRingBuffer>();
const decoder = new TextDecoder();

function hashSubId(sub_id: string): number {
	const target = sub_id.includes('_') ? (sub_id.split('_')[1] ?? '') : sub_id;
	let hash = 0;
	for (let i = 0; i < target.length; i++) hash = (hash << 5) - hash + target.charCodeAt(i);
	return Math.abs(hash) % outputRings.length;
}

function getOutRingForSubId(subId: string): ByteRingBuffer {
	let ring = subIdToRing.get(subId);
	if (ring) return ring;
	ring = outputRings[hashSubId(subId)];
	subIdToRing.set(subId, ring);
	return ring;
}

let lastRingIndex = 0;
let backoffMs = 10;
const MIN_BACKOFF_MS = 10;
const MAX_BACKOFF_MS = 1000;
let loopTimer: number | null = null;

function scheduleLoop() {
	loopTimer = setTimeout(processLoop, backoffMs) as unknown as number;
}

function processLoop() {
	let processed = 0;
	const ringCount = inputRings.length;
	if (ringCount === 0) {
		backoffMs = Math.min(backoffMs * 2, MAX_BACKOFF_MS);
		return scheduleLoop();
	}

	let madeProgress: boolean;
	do {
		madeProgress = false;
		for (let i = 0; i < ringCount; i++) {
			const idx = (lastRingIndex + i) % ringCount;
			const ring = inputRings[idx];
			const record = ring.read();
			if (!record) continue;

			madeProgress = true;
			processed++;

			const envelopeStr = decoder.decode(record);
			let envelope: any;
			try {
				envelope = JSON.parse(envelopeStr);
			} catch {
				continue;
			}
			if (!Array.isArray(envelope.relays) || !Array.isArray(envelope.frames)) continue;

			registry?.sendToRelays(envelope.relays, envelope.frames).catch(console.error);
			lastRingIndex = (idx + 1) % ringCount;
		}
	} while (madeProgress);

	backoffMs = processed > 0 ? MIN_BACKOFF_MS : Math.min(backoffMs * 2, MAX_BACKOFF_MS);
	scheduleLoop();
}

self.addEventListener('message', (evt: MessageEvent<InitMsg>) => {
	const msg = evt.data;
	if (!msg || msg.type === 'init') {
		inputRings = msg.payload.inRings.map((s) => new ByteRingBuffer(s));
		outputRings = msg.payload.outRings.map((s) => new ByteRingBuffer(s));
		registry = new ConnectionRegistry(msg.payload.relayConfig || {});

		const onIncoming = (url: string, subId: string | null, rawText: string) => {
			const sid = subId ?? extractSubIdFast(rawText);
			if (sid) writeEnvelope(getOutRingForSubId(sid), url, rawText);
		};

		const originalEnsure = registry.ensureConnection.bind(registry);
		registry.ensureConnection = async (url: string) => {
			const conn = await originalEnsure(url);
			if (!conn.messageHandler) conn.setMessageHandler(onIncoming);
			return conn;
		};

		// Kick off the polling loop (no Atomics)
		backoffMs = MIN_BACKOFF_MS;
		if (loopTimer !== null) clearTimeout(loopTimer as any);
		// scheduleLoop();
		return;
	}

	// NEW: minimal wake signal to reset backoff and run soon
	if (msg?.type === 'wake') {
		// ensure these are in scope: backoffMs, MIN_BACKOFF_MS, loopTimer, scheduleLoop
		backoffMs = MIN_BACKOFF_MS;
		if (loopTimer !== null) {
			clearTimeout(loopTimer as any);
			loopTimer = null;
		}
		scheduleLoop();
		return;
	}
});
