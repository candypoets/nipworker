import { Event, EventTemplate, NostrEvent } from 'nostr-tools';
import { SharedBufferReader } from 'src/lib/SharedBuffer';
import { WorkerMessage } from './generated/nostr/fb';
import { ByteRingBuffer } from './ws/ring-buffer'; // existing helper
import { RequestObject, type SubscriptionConfig } from './manager';
import { nipWorker, statusRing } from '.';

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
		console.warn('useSharedSubscription: No subscription ID provided');
		return () => {};
	}

	let buffer: SharedArrayBuffer | null = null;
	let lastReadPos: number = 4;
	let running = true;
	let hasUnsubscribed = false;
	let hasSubscribed = false;

	subId = nipWorker.createShortId(subId);
	const manager = nipWorker.getManager(subId);

	const unsubscribe = (): (() => void) => {
		running = false;

		if (hasSubscribed && !hasUnsubscribed) {
			manager.removeEventListener(`subscription:${subId}`, processEvents);
			manager.unsubscribe(subId);
			hasUnsubscribed = true;
		}

		return () => {};
	};

	// nipWorker.resetInputLoopBackoff();
	buffer = manager.subscribe(subId, requests, options);
	hasSubscribed = true;

	const processEvents = (): void => {
		if (!running || !buffer) return;

		let result = SharedBufferReader.readMessages(buffer, lastReadPos);
		while (result.hasNewData) {
			for (const message of result.messages) {
				callback(message);
			}
			lastReadPos = result.newReadPosition;
			// queueMicrotask(() => {
			result = SharedBufferReader.readMessages(buffer, lastReadPos);
			// });
		}
	};

	manager.addEventListener(`subscription:${subId}`, processEvents);

	queueMicrotask(processEvents);

	return unsubscribe;
}

export function usePublish(
	pubId: string,
	event: EventTemplate,
	callback: (message: WorkerMessage) => void = () => {},
	options: { trackStatus?: boolean } = { trackStatus: true }
): () => void {
	if (!pubId) {
		console.warn('usePublish: No publish ID provided');
		return () => {};
	}

	let buffer: SharedArrayBuffer | null = null;
	let lastReadPos: number = 4;
	let running = true;

	pubId = nipWorker.createShortId(pubId);

	const manager = nipWorker.getManager(pubId);

	const unsubscribe = (): void => {
		running = false;
		manager.removeEventListener(`publish:${pubId}`, processEvents);
	};

	buffer = manager.publish(pubId, event);

	const processEvents = (): void => {
		if (!running || !buffer) {
			return;
		}
		const result = SharedBufferReader.readMessages(buffer, lastReadPos);
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
	const manager = nipWorker.getManager('');

	manager.signEvent(template, callback);
}
