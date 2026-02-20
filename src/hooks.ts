import { Event, EventTemplate } from 'nostr-tools';
import { ArrayBufferReader } from 'src/lib/ArrayBufferReader';
// import { nipWorker, statusRing } from '.';
import { WorkerMessage } from './generated/nostr/fb';
import { RequestObject, statusRing, manager, type SubscriptionConfig } from '.';
import { ByteRingBuffer } from './ws/ring-buffer'; // existing helper
// Re-export type guard utilities for hooks users
export {
	isParsedEvent,
	isNostrEvent,
	isConnectionStatus,
	isEoce,
	asParsedEvent,
	asNostrEvent
} from './lib/NarrowTypes';

const decoder = new TextDecoder();

// Provide a handler from your app (e.g., React setState or Redux dispatch)
export type StatusHandler = (status: 'connected' | 'failed' | 'close', url: string) => void;

export function useRelayStatus(onStatus: StatusHandler) {
	const ring = new ByteRingBuffer(statusRing);

	let stopped = false;
	let backoffMs = 10;
	const MIN_BACKOFF_MS = 10;
	const MAX_BACKOFF_MS = 100;

	const loop = async () => {
		while (!stopped) {
			let processed = 0;

			while (true) {
				const record = ring.read();
				if (!record) break;

				processed++;
				const line = decoder.decode(record);
				const sep = line.indexOf('|');
				if (sep > 0) {
					const status = line.slice(0, sep) as 'connected' | 'failed' | 'close';
					const url = line.slice(sep + 1);
					if (status === 'connected' || status === 'failed' || status === 'close') {
						onStatus(status, url);
					}
				}
			}

			backoffMs = processed > 0 ? MIN_BACKOFF_MS : Math.min(backoffMs * 2, MAX_BACKOFF_MS);
			await new Promise((r) => setTimeout(r, backoffMs));
		}
	};

	// Fire-and-forget
	loop();

	// Return a stop handle
	return () => {
		stopped = true;
	};
}

export function useSubscription(
	subId: string,
	requests: RequestObject[],
	callback: (message: WorkerMessage) => void = () => {},
	options: SubscriptionConfig = { closeOnEose: false }
): () => void {
	if (!subId) {
		return () => {};
	}

	let buffer: ArrayBuffer | null = null;
	let lastReadPos = 4;
	let running = true;
	let hasUnsubscribed = false;
	let hasSubscribed = false;
	subId = manager.createShortId(subId);

	// Reentrancy/coalescing flags
	let scheduled = false;
	let processing = false;

	const processEvents = (): void => {
		if (!running || !buffer) {
			processing = false;
			return;
		}
		if (processing) return;
		processing = true;
		try {
			let result = ArrayBufferReader.readMessages(buffer, lastReadPos);
			while (result.hasNewData && buffer) {
				for (const message of result.messages) {
					callback(message);
				}
				lastReadPos = result.newReadPosition;
				result = ArrayBufferReader.readMessages(buffer, lastReadPos);
			}
		} catch (e) {
			console.error('[useSubscription processEvents] error:', e);
		} finally {
			processing = false;
		}
	};

	const scheduleProcess = () => {
		if (!running) return;
		if (scheduled) return;
		scheduled = true;
		queueMicrotask(() => {
			scheduled = false;
			processEvents();
		});
	};

	const unsubscribe = (): void => {
		if (hasUnsubscribed) return;
		running = false;
		if (hasSubscribed) {
			manager.removeEventListener(`subscription:${subId}`, scheduleProcess);
			manager.unsubscribe(subId);
			hasUnsubscribed = true;
		}
		Promise.resolve().then(() => {
			buffer = null;
			lastReadPos = 4;
		});
	};

	buffer = manager.subscribe(subId, requests, options);
	hasSubscribed = true;
	manager.addEventListener(`subscription:${subId}`, scheduleProcess);
	scheduleProcess();

	return unsubscribe;
}

export function usePublish(
	pubId: string,
	event: EventTemplate,
	callback: (message: WorkerMessage) => void = () => {},
	options: { trackStatus?: boolean; defaultRelays?: string[] } = {
		trackStatus: true,
		defaultRelays: []
	}
): () => void {
	if (!pubId) {
		return () => {};
	}

	let buffer: ArrayBuffer | null = null;
	let lastReadPos: number = 4;
	let running = true;

	// pubId = nipWorker.createShortId(pubId);

	// const manager = nipWorker.getManager(pubId);

	const unsubscribe = (): void => {
		running = false;
		manager.removeEventListener(`publish:${pubId}`, processEvents);
	};

	buffer = manager.publish(pubId, event as any, options.defaultRelays);

	const processEvents = (): void => {
		if (!running || !buffer) {
			return;
		}
		const result = ArrayBufferReader.readMessages(buffer, lastReadPos);
		if (result.hasNewData) {
			result.messages.forEach((message: WorkerMessage) => {
				callback(message);
			});
			lastReadPos = result.newReadPosition;
		}
	};

	if (options.trackStatus) {
		manager.addEventListener(`publish:${pubId}`, processEvents);
		queueMicrotask(processEvents);
	}

	return unsubscribe;
}

export function useSignEvent(template: EventTemplate, callback: (event: Event) => void) {
	// const manager = nipWorker.getManager('');

	manager.signEvent(template, callback);
}
