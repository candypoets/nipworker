import { Event, EventTemplate, NostrEvent } from 'nostr-tools';
import { SharedBufferReader } from 'src/lib/SharedBuffer';
import { WorkerMessage } from './generated/nostr/fb';
import { RequestObject, type SubscriptionConfig } from './manager';
import { nipWorker } from '.';

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
