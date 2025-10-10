import { Event, EventTemplate, NostrEvent } from 'nostr-tools';
import { SharedBufferReader } from 'src/lib/SharedBuffer';
import { WorkerMessage } from './generated/nostr/fb';
import { RequestObject, type SubscriptionConfig } from './manager';
import { nipWorker } from '.';

// Global cold-start timestamp and per-frame token bucket shared by all subscriptions in this module.
const APP_START_TS = Date.now();

// Global tokens replenished once per frame. All useSubscription instances share this.
let TOKENS_PER_FRAME = 0;
let tokensRemaining = 0;
let tokenRefillScheduled = false;

function inColdStart(coldMs = 2000) {
	return Date.now() - APP_START_TS < coldMs;
}

function scheduleTokenRefill() {
	if (tokenRefillScheduled) return;
	tokenRefillScheduled = true;
	const raf =
		typeof requestAnimationFrame === 'function'
			? requestAnimationFrame
			: (fn: FrameRequestCallback) =>
					setTimeout(() => fn(performance?.now?.() ?? Date.now()), 16) as unknown as number;

	raf(() => {
		// Refill once per frame with cold/hot limits
		TOKENS_PER_FRAME = inColdStart() ? 200 : 1500; // tune these
		tokensRemaining = TOKENS_PER_FRAME;
		tokenRefillScheduled = false;
		// We don't call any flushers here; each subscription already schedules its own flush.
	});
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

	// Per-instance queues and scheduling
	const MAX_PENDING_BUFFER = 5000; // cap per-subscription memory
	const READ_BUDGET_MS_COLD = 4;
	const READ_BUDGET_MS_HOT = 8;

	let processing = false;
	let pending: WorkerMessage[] = [];
	let flushScheduled = false;
	let rafHandle: number | null = null;
	let idleHandle: number | null = null;
	let sliceTimeout: ReturnType<typeof setTimeout> | null = null;

	// Polyfills
	const raf =
		typeof requestAnimationFrame === 'function'
			? requestAnimationFrame
			: (fn: FrameRequestCallback) =>
					setTimeout(() => fn(performance?.now?.() ?? Date.now()), 16) as unknown as number;

	const caf =
		typeof cancelAnimationFrame === 'function'
			? cancelAnimationFrame
			: (id: number) => clearTimeout(id as unknown as number);

	// Minimal typings for requestIdleCallback
	declare const window: any;

	const pushPending = (msg: WorkerMessage) => {
		if (pending.length >= MAX_PENDING_BUFFER) {
			// Drop oldest to keep memory bounded during surges
			const drop = pending.length - MAX_PENDING_BUFFER + 1;
			if (drop > 0) pending.splice(0, drop);
		}
		pending.push(msg);
	};

	const flushSomeWithGlobalTokens = () => {
		if (!running || pending.length === 0) return;

		// Ensure tokens will be refilled this or next frame
		scheduleTokenRefill();

		// Nothing to do if global tokens are out; try again next frame
		if (tokensRemaining <= 0) {
			scheduleFlush(); // reschedule for next frame when tokens are refilled
			return;
		}

		// Deliver up to the available global tokens
		const deliverCount = Math.min(tokensRemaining, pending.length);
		if (deliverCount <= 0) return;

		const chunk = pending.splice(0, deliverCount);
		tokensRemaining -= deliverCount;

		for (const msg of chunk) {
			if (!running) break;
			callback(msg);
		}

		// If we still have backlog, schedule another flush (may run next frame if tokens are depleted)
		if (pending.length > 0) scheduleFlush();
	};

	const scheduleFlush = () => {
		if (flushScheduled || !running) return;
		flushScheduled = true;

		// During cold start, prefer idle time if available to avoid competing with first paint/hydration
		if (
			inColdStart() &&
			typeof window !== 'undefined' &&
			typeof window.requestIdleCallback === 'function'
		) {
			idleHandle = window.requestIdleCallback!(
				() => {
					idleHandle = null;
					flushScheduled = false;
					flushSomeWithGlobalTokens();
				},
				{ timeout: 100 }
			);
			return;
		}

		rafHandle = raf(() => {
			flushScheduled = false;
			rafHandle = null;
			flushSomeWithGlobalTokens();
		});
	};

	subId = nipWorker.createShortId(subId);
	const manager = nipWorker.getManager(subId);

	const unsubscribe = (): (() => void) => {
		running = false;

		if (rafHandle != null) {
			caf(rafHandle);
			rafHandle = null;
		}
		if (idleHandle != null && typeof window?.cancelIdleCallback === 'function') {
			window.cancelIdleCallback(idleHandle);
			idleHandle = null;
		}
		if (sliceTimeout) {
			clearTimeout(sliceTimeout);
			sliceTimeout = null;
		}

		if (hasSubscribed && !hasUnsubscribed) {
			manager.removeEventListener(`subscription:${subId}`, processEvents);
			manager.unsubscribe(subId);
			hasUnsubscribed = true;
		}

		pending = [];
		return () => {};
	};
	// console.log('no more wake');
	// nipWorker.resetInputLoopBackoff();
	buffer = manager.subscribe(subId, requests, options);
	hasSubscribed = true;

	const processEvents = (): void => {
		if (!running || !buffer) return;
		if (processing) return;

		processing = true;

		const start = performance?.now?.() ?? Date.now();
		const budget = inColdStart() ? READ_BUDGET_MS_COLD : READ_BUDGET_MS_HOT;

		let result = SharedBufferReader.readMessages(buffer, lastReadPos);
		while (result.hasNewData) {
			for (const message of result.messages) pushPending(message);
			lastReadPos = result.newReadPosition;

			const now = performance?.now?.() ?? Date.now();
			if (now - start >= budget) {
				processing = false;
				scheduleFlush();
				// Yield to keep the main thread responsive
				sliceTimeout = setTimeout(() => {
					sliceTimeout = null;
					processEvents();
				}, 0);
				return;
			}

			// If we’re already backed up, stop reading for now and let the UI catch up
			if (pending.length >= MAX_PENDING_BUFFER) {
				processing = false;
				scheduleFlush();
				return;
			}

			result = SharedBufferReader.readMessages(buffer, lastReadPos);
		}

		processing = false;
		scheduleFlush();
	};

	manager.addEventListener(`subscription:${subId}`, processEvents);

	// Kick off initial read without blocking the current stack
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
