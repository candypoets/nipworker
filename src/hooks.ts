import { Event, EventTemplate } from 'nostr-tools';
import { ArrayBufferReader } from 'src/lib/ArrayBufferReader';
import { WorkerMessage } from './generated/nostr/fb';
import { RequestObject, manager, type SubscriptionConfig } from '.';
// Re-export type guard utilities for hooks users
export {
	isParsedEvent,
	isNostrEvent,
	isConnectionStatus,
	isEoce,
	asParsedEvent,
	asNostrEvent
} from './lib/NarrowTypes';

// Provide a handler from your app (e.g., React setState or Redux dispatch)
export type StatusHandler = (status: 'connected' | 'failed' | 'close', url: string) => void;

/**
 * Hook to receive relay connection status updates.
 * Immediately calls handler with current statuses, then subscribes to real-time updates.
 */
export function useRelayStatus(onStatus: StatusHandler) {
	let stopped = false;

	// Get current statuses immediately
	const statuses = manager.getRelayStatuses();
	for (const [url, { status }] of statuses) {
		onStatus(status, url);
	}

	// Handler for real-time updates
	const handleStatus = (event: globalThis.Event) => {
		if (stopped) return;
		const customEvent = event as globalThis.CustomEvent<{ status: 'connected' | 'failed' | 'close'; url: string }>;
		const { status, url } = customEvent.detail;
		onStatus(status, url);
	};

	// Subscribe to updates
	manager.addEventListener('relay:status', handleStatus as globalThis.EventListener);

	// Return a stop handle
	return () => {
		stopped = true;
		manager.removeEventListener('relay:status', handleStatus as globalThis.EventListener);
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
