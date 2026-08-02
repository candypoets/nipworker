import type { WorkerMessage } from '../generated/nostr/fb';
import type { RequestObject, SubscriptionConfig } from '../types';
import { isConnectionStatus } from './NarrowTypes';
import { createTimeWindowPager, type TimeWindowPage } from './TimeWindowPager';

export type PaginatedSubscriptionState = {
	loading: boolean;
	hasMore: boolean;
};

export type PaginatedSubscription = {
	readonly state: PaginatedSubscriptionState;
	start(): void;
	loadMore(): boolean;
	close(): void;
};

export type PaginatedMessageHandler = (message: WorkerMessage) => number | undefined | void;

export type SubscriptionFactory = (
	subId: string,
	requests: RequestObject[],
	callback: (message: WorkerMessage) => void,
	options?: SubscriptionConfig
) => () => void;

export type PaginatedSubscriptionConfig = {
	subId: string;
	/** Requests used by the long-running root subscription. */
	requests: readonly RequestObject[];
	/** Optional historical requests when root-only bounds or cache options should not be paged. */
	pageRequests?: readonly RequestObject[];
	onMessage: PaginatedMessageHandler;
	windowSeconds: number;
	anchor?: number;
	maxEmptyPages?: number;
	emptyWindowGrowthFactor?: number;
	/** Build subscription options for both the root and each page subId. */
	options?: SubscriptionConfig | ((subId: string) => SubscriptionConfig);
	/** Initial root loading fallback. Defaults to 3 seconds. */
	rootTimeoutMs?: number;
	/** Whether start() reports loading while the root resolves. Defaults to true. */
	initialLoading?: boolean;
	/** Page completion fallback. Defaults to 10 seconds. */
	pageTimeoutMs?: number;
	/** Time allowed for events queued behind the final EOSE. Defaults to 500ms. */
	eoseDrainMs?: number;
	onStateChange?: (state: PaginatedSubscriptionState) => void;
};

function nonNegativeInteger(value: number, name: string): number {
	if (!Number.isSafeInteger(value) || value < 0) {
		throw new RangeError(`${name} must be a non-negative integer`);
	}
	return value;
}

function relayKey(relay: string): string {
	try {
		const url = new URL(relay);
		url.hash = '';
		url.search = '';
		url.pathname = url.pathname.replace(/\/+$/, '');
		return url.toString().replace(/\/$/, '');
	} catch {
		return relay.replace(/\/+$/, '');
	}
}

function expectedRelays(requests: readonly RequestObject[]): Set<string> {
	return new Set(requests.flatMap((request) => request.relays.map(relayKey)));
}

function optionsFor(
	options: PaginatedSubscriptionConfig['options'],
	subId: string
): SubscriptionConfig {
	return typeof options === 'function' ? options(subId) : { ...options };
}

/**
 * Own a long-running root subscription and its bounded backward page subscriptions.
 * The message handler remains responsible for accepting events into application state;
 * returning an accepted event timestamp lets the controller advance the page cursor.
 */
export function createPaginatedSubscriptionController(
	config: PaginatedSubscriptionConfig,
	subscribe: SubscriptionFactory
): PaginatedSubscription {
	const rootTimeoutMs = nonNegativeInteger(config.rootTimeoutMs ?? 3_000, 'rootTimeoutMs');
	const pageTimeoutMs = nonNegativeInteger(config.pageTimeoutMs ?? 10_000, 'pageTimeoutMs');
	const eoseDrainMs = nonNegativeInteger(config.eoseDrainMs ?? 500, 'eoseDrainMs');
	const pager = createTimeWindowPager({
		subId: config.subId,
		requests: config.pageRequests ?? config.requests,
		windowSeconds: config.windowSeconds,
		...(config.anchor === undefined ? {} : { anchor: config.anchor }),
		...(config.maxEmptyPages === undefined ? {} : { maxEmptyPages: config.maxEmptyPages }),
		...(config.emptyWindowGrowthFactor === undefined
			? {}
			: { emptyWindowGrowthFactor: config.emptyWindowGrowthFactor })
	});

	let state: PaginatedSubscriptionState = { loading: false, hasMore: true };
	let started = false;
	let closed = false;
	let paginationStarted = false;
	let rootPending = false;
	let rootOldestCreatedAt: number | undefined;
	let rootUnsubscribe: (() => void) | undefined;
	let rootTimeout: ReturnType<typeof setTimeout> | undefined;
	let rootDrainTimeout: ReturnType<typeof setTimeout> | undefined;
	let rootEoseRelays = new Set<string>();
	const rootExpectedRelays = expectedRelays(config.requests);

	let activePage: TimeWindowPage | undefined;
	let pageOldestCreatedAt: number | undefined;
	let pageUnsubscribe: (() => void) | undefined;
	let pageTimeout: ReturnType<typeof setTimeout> | undefined;
	let pageDrainTimeout: ReturnType<typeof setTimeout> | undefined;
	let pageExpectedRelays = new Set<string>();
	let pageEoseRelays = new Set<string>();

	function setState(next: PaginatedSubscriptionState) {
		state = next;
		config.onStateChange?.({ ...state });
	}

	function clearRootTimers() {
		if (rootTimeout) clearTimeout(rootTimeout);
		if (rootDrainTimeout) clearTimeout(rootDrainTimeout);
		rootTimeout = undefined;
		rootDrainTimeout = undefined;
	}

	function clearPageTimers() {
		if (pageTimeout) clearTimeout(pageTimeout);
		if (pageDrainTimeout) clearTimeout(pageDrainTimeout);
		pageTimeout = undefined;
		pageDrainTimeout = undefined;
	}

	function finishRootLoading() {
		if (!rootPending) return;
		rootPending = false;
		clearRootTimers();
		if (!activePage) setState({ ...state, loading: false });
	}

	function recordEose(
		message: WorkerMessage,
		expected: Set<string>,
		received: Set<string>
	): boolean {
		const status = isConnectionStatus(message);
		const relayUrl = status?.relayUrl();
		if (status?.status() !== 'EOSE' || !relayUrl) return false;
		received.add(relayKey(relayUrl));
		return expected.size > 0 && Array.from(expected).every((relay) => received.has(relay));
	}

	function handleRootMessage(message: WorkerMessage) {
		const acceptedAt = config.onMessage(message);
		if (acceptedAt !== undefined) {
			if (
				Number.isSafeInteger(acceptedAt) &&
				acceptedAt >= pager.anchor - pager.windowSeconds &&
				acceptedAt < pager.anchor
			) {
				rootOldestCreatedAt = Math.min(rootOldestCreatedAt ?? acceptedAt, acceptedAt);
			}
			finishRootLoading();
		}

		if (rootPending && recordEose(message, rootExpectedRelays, rootEoseRelays)) {
			if (!rootDrainTimeout) {
				rootDrainTimeout = setTimeout(finishRootLoading, eoseDrainMs);
			}
		}
	}

	function settlePage() {
		if (!activePage) return;
		clearPageTimers();
		pageUnsubscribe?.();
		pageUnsubscribe = undefined;

		const completion = pager.complete(pageOldestCreatedAt);
		activePage = undefined;
		pageOldestCreatedAt = undefined;
		pageExpectedRelays = new Set<string>();
		pageEoseRelays = new Set<string>();

		if (completion.shouldRetry) {
			startNextPage();
			return;
		}
		setState({ loading: false, hasMore: completion.hasMore });
	}

	function handlePageMessage(message: WorkerMessage) {
		const page = activePage;
		if (!page) return;

		const acceptedAt = config.onMessage(message);
		if (
			acceptedAt !== undefined &&
			Number.isSafeInteger(acceptedAt) &&
			acceptedAt >= page.window.since &&
			acceptedAt <= page.window.until
		) {
			pageOldestCreatedAt = Math.min(pageOldestCreatedAt ?? acceptedAt, acceptedAt);
		}

		if (recordEose(message, pageExpectedRelays, pageEoseRelays)) {
			if (!pageDrainTimeout) pageDrainTimeout = setTimeout(settlePage, eoseDrainMs);
		}
	}

	function startNextPage(): boolean {
		const page = pager.next();
		if (!page) {
			activePage = undefined;
			setState({ loading: false, hasMore: false });
			return false;
		}

		activePage = page;
		pageOldestCreatedAt = undefined;
		pageExpectedRelays = expectedRelays(page.requests);
		pageEoseRelays = new Set<string>();
		setState({ loading: true, hasMore: true });
		pageUnsubscribe = subscribe(page.subId, page.requests, handlePageMessage, {
			...optionsFor(config.options, page.subId),
			...page.options
		});
		pageTimeout = setTimeout(settlePage, pageTimeoutMs);
		return true;
	}

	return {
		get state() {
			return state;
		},
		start() {
			if (started || closed) return;
			started = true;
			rootPending = true;
			setState({ loading: config.initialLoading ?? true, hasMore: true });
			rootUnsubscribe = subscribe(
				config.subId,
				config.requests.map((request) => ({ ...request })),
				handleRootMessage,
				optionsFor(config.options, config.subId)
			);
			if (rootPending) rootTimeout = setTimeout(finishRootLoading, rootTimeoutMs);
		},
		loadMore() {
			if (!started || closed || rootPending || activePage || !state.hasMore) return false;
			if (!paginationStarted) {
				if (rootOldestCreatedAt !== undefined) pager.reset(rootOldestCreatedAt);
				paginationStarted = true;
			}
			return startNextPage();
		},
		close() {
			if (closed) return;
			closed = true;
			rootPending = false;
			clearRootTimers();
			clearPageTimers();
			rootUnsubscribe?.();
			pageUnsubscribe?.();
			rootUnsubscribe = undefined;
			pageUnsubscribe = undefined;
			activePage = undefined;
			setState({ loading: false, hasMore: false });
		}
	};
}
