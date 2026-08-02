import type { RequestObject } from '../types';

export type TimeWindow = {
	index: number;
	since: number;
	until: number;
};

export type TimeWindowPage = {
	subId: string;
	requests: RequestObject[];
	window: TimeWindow;
	options: { pagination: string };
};

export type TimeWindowLiveSubscription = {
	subId: string;
	requests: RequestObject[];
};

export type TimeWindowPageCompletion = {
	hasMore: boolean;
	shouldRetry: boolean;
	consecutiveEmptyPages: number;
};

export type TimeWindowPager = {
	readonly rootSubId: string;
	readonly anchor: number;
	readonly windowSeconds: number;
	live(): TimeWindowLiveSubscription;
	page(index: number): TimeWindowPage | null;
	next(): TimeWindowPage | null;
	complete(oldestReceivedAt?: number): TimeWindowPageCompletion;
	reset(anchor?: number): void;
};

export type TimeWindowPagerConfig = {
	subId: string;
	requests: readonly RequestObject[];
	/** First second owned by the live subscription. Defaults to the current Unix time. */
	anchor?: number;
	windowSeconds: number;
	/** Consecutive empty page attempts before history is considered exhausted. Defaults to 1. */
	maxEmptyPages?: number;
	/** Multiplier applied to the next older window after each empty page. Defaults to 2. */
	emptyWindowGrowthFactor?: number;
};

function unixSecond(value: number, name: string): number {
	if (!Number.isSafeInteger(value) || value < 0) {
		throw new RangeError(`${name} must be a non-negative Unix timestamp`);
	}
	return value;
}

function positiveInteger(value: number, name: string): number {
	if (!Number.isSafeInteger(value) || value <= 0) {
		throw new RangeError(`${name} must be a positive integer`);
	}
	return value;
}

function pageIndex(value: number): number {
	if (!Number.isSafeInteger(value) || value < 0) {
		throw new RangeError('page index must be a non-negative integer');
	}
	return value;
}

/** Build the deterministic, inclusive time window for a backward page. */
export function timeWindowForPage(
	anchor: number,
	windowSeconds: number,
	index: number
): TimeWindow | null {
	const safeAnchor = unixSecond(anchor, 'anchor');
	const safeWindowSeconds = positiveInteger(windowSeconds, 'windowSeconds');
	const safeIndex = pageIndex(index);
	const until = safeAnchor - safeWindowSeconds * safeIndex - 1;
	if (until < 0) return null;

	return {
		index: safeIndex,
		since: Math.max(0, until - safeWindowSeconds + 1),
		until
	};
}

/**
 * Apply an inclusive time window without mutating the caller's filters.
 * Existing since/until values remain hard lower/upper bounds.
 */
export function applyTimeWindow(
	requests: readonly RequestObject[],
	window: Pick<TimeWindow, 'since' | 'until'>
): RequestObject[] {
	const windowSince = unixSecond(window.since, 'window.since');
	const windowUntil = unixSecond(window.until, 'window.until');
	if (windowSince > windowUntil) {
		throw new RangeError('window.since must not be greater than window.until');
	}

	return requests.flatMap((request) => {
		const since = Math.max(request.since ?? 0, windowSince);
		const until = Math.min(request.until ?? Number.MAX_SAFE_INTEGER, windowUntil);
		return since <= until ? [{ ...request, since, until }] : [];
	});
}

/**
 * Create a bounded backward pager. A completed page continues from its oldest event
 * until the current window is exhausted. Empty pages move to progressively larger
 * older windows, up to maxEmptyPages consecutive attempts.
 *
 * The root subscription must remain alive while pages share its deduplication state.
 */
export function createTimeWindowPager(config: TimeWindowPagerConfig): TimeWindowPager {
	if (!config.subId) throw new TypeError('subId is required');

	let anchor = unixSecond(config.anchor ?? Math.floor(Date.now() / 1000), 'anchor');
	const windowSeconds = positiveInteger(config.windowSeconds, 'windowSeconds');
	const maxEmptyPages = positiveInteger(config.maxEmptyPages ?? 1, 'maxEmptyPages');
	const emptyWindowGrowthFactor = positiveInteger(
		config.emptyWindowGrowthFactor ?? 2,
		'emptyWindowGrowthFactor'
	);
	let nextIndex: number;
	let nextSince: number;
	let nextUntil: number;
	let activePage: TimeWindowPage | null;
	let consecutiveEmptyPages: number;
	let exhausted: boolean;

	function resetState() {
		nextIndex = 0;
		nextUntil = anchor - 1;
		nextSince = Math.max(0, nextUntil - windowSeconds + 1);
		activePage = null;
		consecutiveEmptyPages = 0;
		exhausted = nextUntil < 0;
	}

	function moveBefore(since: number, duration: number) {
		nextUntil = since - 1;
		if (nextUntil < 0) {
			exhausted = true;
			return;
		}
		nextSince = Math.max(0, nextUntil - duration + 1);
	}

	function grownWindowDuration(): number {
		let duration = windowSeconds;
		const availableHistory = nextUntil + 1;
		for (let index = 0; index < consecutiveEmptyPages; index += 1) {
			duration = Math.min(availableHistory, duration * emptyWindowGrowthFactor);
		}
		return duration;
	}

	resetState();

	return {
		get rootSubId() {
			return config.subId;
		},
		get anchor() {
			return anchor;
		},
		get windowSeconds() {
			return windowSeconds;
		},
		live() {
			return {
				subId: config.subId,
				requests: config.requests.flatMap((request) => {
					const since = Math.max(request.since ?? 0, anchor);
					return request.until === undefined || since <= request.until
						? [{ ...request, since }]
						: [];
				})
			};
		},
		page(index) {
			const window = timeWindowForPage(anchor, windowSeconds, index);
			if (!window) return null;
			const requests = applyTimeWindow(config.requests, window);
			if (requests.length === 0) return null;

			return {
				subId: `${config.subId}:page:${window.index}:${window.since}:${window.until}`,
				requests,
				window,
				options: { pagination: config.subId }
			};
		},
		next() {
			if (exhausted) return null;
			if (activePage) return activePage;

			const window = { index: nextIndex, since: nextSince, until: nextUntil };
			const requests = applyTimeWindow(config.requests, window);
			if (requests.length === 0) {
				exhausted = true;
				return null;
			}

			activePage = {
				subId: `${config.subId}:page:${window.index}:${window.since}:${window.until}`,
				requests,
				window,
				options: { pagination: config.subId }
			};
			nextIndex += 1;
			return activePage;
		},
		complete(oldestReceivedAt) {
			if (!activePage) throw new Error('no active page to complete');

			const completedWindow = activePage.window;
			activePage = null;

			if (oldestReceivedAt !== undefined) {
				const oldest = unixSecond(oldestReceivedAt, 'oldestReceivedAt');
				if (oldest < completedWindow.since || oldest > completedWindow.until) {
					throw new RangeError('oldestReceivedAt must be inside the completed page window');
				}

				consecutiveEmptyPages = 0;
				if (oldest > completedWindow.since) {
					nextSince = completedWindow.since;
					nextUntil = oldest - 1;
				} else {
					moveBefore(completedWindow.since, windowSeconds);
				}
			} else {
				consecutiveEmptyPages += 1;
				if (consecutiveEmptyPages >= maxEmptyPages) {
					exhausted = true;
				} else {
					moveBefore(completedWindow.since, windowSeconds);
					if (!exhausted) nextSince = Math.max(0, nextUntil - grownWindowDuration() + 1);
				}
			}

			return {
				hasMore: !exhausted,
				shouldRetry: oldestReceivedAt === undefined && !exhausted,
				consecutiveEmptyPages
			};
		},
		reset(nextAnchor = Math.floor(Date.now() / 1000)) {
			anchor = unixSecond(nextAnchor, 'anchor');
			resetState();
		}
	};
}
