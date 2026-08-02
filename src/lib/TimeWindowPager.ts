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

export type TimeWindowPager = {
	readonly rootSubId: string;
	readonly anchor: number;
	readonly windowSeconds: number;
	live(): TimeWindowLiveSubscription;
	page(index: number): TimeWindowPage | null;
	next(): TimeWindowPage | null;
	reset(anchor?: number): void;
};

export type TimeWindowPagerConfig = {
	subId: string;
	requests: readonly RequestObject[];
	/** First second owned by the live subscription. Defaults to the current Unix time. */
	anchor?: number;
	windowSeconds: number;
};

function unixSecond(value: number, name: string): number {
	if (!Number.isSafeInteger(value) || value < 0) {
		throw new RangeError(`${name} must be a non-negative Unix timestamp`);
	}
	return value;
}

function positiveSeconds(value: number): number {
	if (!Number.isSafeInteger(value) || value <= 0) {
		throw new RangeError('windowSeconds must be a positive integer');
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
	const safeWindowSeconds = positiveSeconds(windowSeconds);
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
 * Create a backward pager whose history boundaries never depend on returned events.
 * The root subscription must remain alive while pages share its deduplication state.
 */
export function createTimeWindowPager(config: TimeWindowPagerConfig): TimeWindowPager {
	if (!config.subId) throw new TypeError('subId is required');

	let anchor = unixSecond(config.anchor ?? Math.floor(Date.now() / 1000), 'anchor');
	const windowSeconds = positiveSeconds(config.windowSeconds);
	let nextIndex = 0;

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
			const page = this.page(nextIndex);
			if (page) nextIndex += 1;
			return page;
		},
		reset(nextAnchor = Math.floor(Date.now() / 1000)) {
			anchor = unixSecond(nextAnchor, 'anchor');
			nextIndex = 0;
		}
	};
}
