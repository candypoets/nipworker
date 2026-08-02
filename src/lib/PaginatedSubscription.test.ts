import { beforeEach, describe, expect, it, vi } from 'vitest';

import type { WorkerMessage } from '../generated/nostr/fb';
import type { RequestObject, SubscriptionConfig } from '../types';
import {
	createPaginatedSubscriptionController,
	type SubscriptionFactory
} from './PaginatedSubscription';

vi.mock('./NarrowTypes', () => ({
	isConnectionStatus: (message: { relayUrl?: string; status?: string }) =>
		message.status
			? {
					relayUrl: () => message.relayUrl ?? null,
					status: () => message.status ?? null
				}
			: null
}));

type FakeMessage = {
	acceptedAt?: number;
	relayUrl?: string;
	status?: string;
};

type RecordedSubscription = {
	requests: RequestObject[];
	callback: (message: WorkerMessage) => void;
	options?: SubscriptionConfig;
	closed: boolean;
};

function setup() {
	const subscriptions = new Map<string, RecordedSubscription>();
	const subscribe: SubscriptionFactory = (subId, requests, callback, options) => {
		const subscription = { requests, callback, options, closed: false };
		subscriptions.set(subId, subscription);
		return () => {
			subscription.closed = true;
		};
	};
	const emit = (subId: string, message: FakeMessage) => {
		subscriptions.get(subId)?.callback(message as unknown as WorkerMessage);
	};
	return { subscriptions, subscribe, emit };
}

const requests: RequestObject[] = [{ kinds: [1], limit: 50, relays: ['wss://relay.example.com/'] }];

describe('PaginatedSubscription', () => {
	beforeEach(() => {
		vi.useFakeTimers();
	});

	it('keeps the root alive and uses its oldest recent accepted event as the first cursor', () => {
		const { subscriptions, subscribe, emit } = setup();
		const states: boolean[] = [];
		const controller = createPaginatedSubscriptionController(
			{
				subId: 'feed',
				requests,
				anchor: 1_000,
				windowSeconds: 100,
				initialLoading: false,
				onMessage: (message) => (message as unknown as FakeMessage).acceptedAt,
				onStateChange: (state) => states.push(state.loading),
				options: (subId) => ({ maxEvents: subId.length })
			},
			subscribe
		);

		controller.start();
		expect(controller.loadMore()).toBe(false);
		emit('feed', { acceptedAt: 975 });
		emit('feed', { acceptedAt: 950 });
		expect(controller.loadMore()).toBe(true);

		const page = subscriptions.get('feed:page:0:850:949');
		expect(page?.requests).toEqual([{ ...requests[0], since: 850, until: 949 }]);
		expect(page?.options).toEqual({ maxEvents: 19, pagination: 'feed' });
		expect(subscriptions.get('feed')?.closed).toBe(false);
		expect(states).toEqual([false, false, true]);
	});

	it('continues a dense window from the oldest accepted page event', () => {
		const { subscriptions, subscribe, emit } = setup();
		const controller = createPaginatedSubscriptionController(
			{
				subId: 'dense',
				requests,
				anchor: 1_000,
				windowSeconds: 100,
				eoseDrainMs: 0,
				onMessage: (message) => (message as unknown as FakeMessage).acceptedAt
			},
			subscribe
		);

		controller.start();
		emit('dense', { status: 'EOSE', relayUrl: 'wss://relay.example.com' });
		vi.runOnlyPendingTimers();
		controller.loadMore();
		const firstPageId = 'dense:page:0:900:999';
		emit(firstPageId, { acceptedAt: 975 });
		emit(firstPageId, { status: 'EOSE', relayUrl: 'wss://relay.example.com/' });
		vi.runOnlyPendingTimers();

		expect(subscriptions.get(firstPageId)?.closed).toBe(true);
		expect(controller.loadMore()).toBe(true);
		expect(subscriptions.has('dense:page:1:900:974')).toBe(true);
	});

	it('ignores stale timestamps outside the active page and retries an older sparse window', () => {
		const { subscriptions, subscribe, emit } = setup();
		const controller = createPaginatedSubscriptionController(
			{
				subId: 'sparse',
				requests,
				anchor: 1_000,
				windowSeconds: 100,
				maxEmptyPages: 3,
				eoseDrainMs: 0,
				onMessage: (message) => (message as unknown as FakeMessage).acceptedAt
			},
			subscribe
		);

		controller.start();
		emit('sparse', { status: 'EOSE', relayUrl: 'wss://relay.example.com' });
		vi.runOnlyPendingTimers();
		controller.loadMore();
		const firstPageId = 'sparse:page:0:900:999';
		emit(firstPageId, { acceptedAt: 100 });
		emit(firstPageId, { status: 'EOSE', relayUrl: 'wss://relay.example.com' });
		vi.runOnlyPendingTimers();

		expect(subscriptions.get(firstPageId)?.closed).toBe(true);
		expect(subscriptions.has('sparse:page:1:700:899')).toBe(true);
		expect(controller.state).toEqual({ loading: true, hasMore: true });
	});

	it('closes the root and active page together', () => {
		const { subscriptions, subscribe, emit } = setup();
		const controller = createPaginatedSubscriptionController(
			{
				subId: 'close',
				requests,
				anchor: 1_000,
				windowSeconds: 100,
				onMessage: (message) => (message as unknown as FakeMessage).acceptedAt
			},
			subscribe
		);

		controller.start();
		emit('close', { acceptedAt: 950 });
		controller.loadMore();
		controller.close();

		expect(subscriptions.get('close')?.closed).toBe(true);
		expect(subscriptions.get('close:page:0:850:949')?.closed).toBe(true);
		expect(controller.state).toEqual({ loading: false, hasMore: false });
	});
});
