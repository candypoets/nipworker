import { ByteRingBuffer } from './ring-buffer';
import { ConnectionRegistry } from './registry';

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

const MIN_BACKOFF_MS = 10;
const MAX_BACKOFF_MS = 1000;

export type WSRuntimeInit = {
	inRings: SharedArrayBuffer[];
	outRings: SharedArrayBuffer[];
	relayConfig: any;
};

export class WSRuntime {
	private inputRings: ByteRingBuffer[] = [];
	private outputRings: ByteRingBuffer[] = [];
	private registry: ConnectionRegistry | null = null;
	private subIdToRing = new Map<string, ByteRingBuffer>();
	private decoder = new TextDecoder();

	private lastRingIndex = 0;
	private backoffMs = MIN_BACKOFF_MS;
	private loopTimer: ReturnType<typeof setTimeout> | null = null;

	constructor(init: WSRuntimeInit) {
		this.inputRings = init.inRings.map((s) => new ByteRingBuffer(s));
		this.outputRings = init.outRings.map((s) => new ByteRingBuffer(s));
		this.registry = new ConnectionRegistry(init.relayConfig || {});

		const onIncoming = (url: string, subId: string | null, rawText: string) => {
			const sid = subId ?? extractSubIdFast(rawText);
			if (sid) writeEnvelope(this.getOutRingForSubId(sid), url, rawText);
		};

		// Wrap ensureConnection to attach inbound handler once
		const originalEnsure = this.registry.ensureConnection.bind(this.registry);
		this.registry.ensureConnection = async (url: string) => {
			const conn = await originalEnsure(url);
			if (!conn.messageHandler) conn.setMessageHandler(onIncoming);
			return conn;
		};

		// Start polling loop
		this.backoffMs = MIN_BACKOFF_MS;
		this.scheduleLoop();
	}

	public wake(): void {
		this.backoffMs = MIN_BACKOFF_MS;
		if (this.loopTimer !== null) {
			clearTimeout(this.loopTimer);
			this.loopTimer = null;
		}
		this.scheduleLoop();
	}

	public destroy(): void {
		if (this.loopTimer !== null) {
			clearTimeout(this.loopTimer);
			this.loopTimer = null;
		}
		// If ConnectionRegistry has a shutdown/close method in your codebase, call it here.
		// (Not invoking anything here to avoid assumptions.)
		this.subIdToRing.clear();
		this.registry = null;
		this.inputRings = [];
		this.outputRings = [];
	}

	private scheduleLoop = (): void => {
		this.loopTimer = setTimeout(this.processLoop, this.backoffMs);
	};

	private processLoop = (): void => {
		let processed = 0;
		const ringCount = this.inputRings.length;
		if (ringCount === 0) {
			this.backoffMs = Math.min(this.backoffMs * 2, MAX_BACKOFF_MS);
			return this.scheduleLoop();
		}

		let madeProgress: boolean;
		do {
			madeProgress = false;
			for (let i = 0; i < ringCount; i++) {
				const idx = (this.lastRingIndex + i) % ringCount;
				const ring = this.inputRings[idx];
				const record = ring.read();
				if (!record) continue;

				madeProgress = true;
				processed++;

				const envelopeStr = this.decoder.decode(record);
				let envelope: any;
				try {
					envelope = JSON.parse(envelopeStr);
				} catch {
					continue;
				}
				if (!Array.isArray(envelope.relays) || !Array.isArray(envelope.frames)) continue;

				// Fire and forget; if desired you could await to pace the loop
				this.registry?.sendToRelays(envelope.relays, envelope.frames).catch(console.error);
				this.lastRingIndex = (idx + 1) % ringCount;
			}
		} while (madeProgress);

		this.backoffMs = processed > 0 ? MIN_BACKOFF_MS : Math.min(this.backoffMs * 2, MAX_BACKOFF_MS);
		this.scheduleLoop();
	};

	private hashSubId(sub_id: string): number {
		const len = this.outputRings.length || 1;
		const target = sub_id.includes('_') ? (sub_id.split('_')[1] ?? '') : sub_id;
		let hash = 0;
		for (let i = 0; i < target.length; i++) hash = (hash << 5) - hash + target.charCodeAt(i);
		return Math.abs(hash) % len;
	}

	private getOutRingForSubId(subId: string): ByteRingBuffer {
		let ring = this.subIdToRing.get(subId);
		if (ring) return ring;
		ring = this.outputRings[this.hashSubId(subId)];
		this.subIdToRing.set(subId, ring);
		return ring;
	}
}
