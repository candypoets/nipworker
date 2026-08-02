import { describe, expect, it, vi } from 'vitest';

import { applyTimeWindow, createTimeWindowPager, timeWindowForPage } from './TimeWindowPager';

const requests = [
	{
		kinds: [1],
		limit: 50,
		relays: ['wss://relay.example.com']
	}
];

describe('TimeWindowPager', () => {
	it('builds adjacent inclusive backward windows', () => {
		expect(timeWindowForPage(1_000, 100, 0)).toEqual({ index: 0, since: 900, until: 999 });
		expect(timeWindowForPage(1_000, 100, 1)).toEqual({ index: 1, since: 800, until: 899 });
	});

	it('intersects generated windows with caller bounds without mutation', () => {
		const bounded = [{ ...requests[0]!, since: 925, until: 975 }];
		expect(applyTimeWindow(bounded, { since: 900, until: 999 })).toEqual([
			{ ...bounded[0], since: 925, until: 975 }
		]);
		expect(bounded[0]).toEqual({ ...requests[0], since: 925, until: 975 });
	});

	it('creates a live root and stable pages that share its pagination key', () => {
		const pager = createTimeWindowPager({
			subId: 'profile:alice',
			requests,
			anchor: 1_000,
			windowSeconds: 100
		});

		expect(pager.live()).toEqual({
			subId: 'profile:alice',
			requests: [{ ...requests[0], since: 1_000 }]
		});
		expect(pager.next()).toEqual({
			subId: 'profile:alice:page:0:900:999',
			requests: [{ ...requests[0], since: 900, until: 999 }],
			window: { index: 0, since: 900, until: 999 },
			options: { pagination: 'profile:alice' }
		});
		expect(pager.next()?.window).toEqual({ index: 0, since: 900, until: 999 });
		expect(pager.complete(950)).toEqual({
			hasMore: true,
			shouldRetry: false,
			consecutiveEmptyPages: 0
		});
		expect(pager.next()?.window).toEqual({ index: 1, since: 900, until: 949 });
	});

	it('continues dense windows from the oldest received event', () => {
		const pager = createTimeWindowPager({
			subId: 'dense',
			requests,
			anchor: 1_000,
			windowSeconds: 100
		});

		expect(pager.next()?.window).toEqual({ index: 0, since: 900, until: 999 });
		pager.complete(975);
		expect(pager.next()?.window).toEqual({ index: 1, since: 900, until: 974 });
		pager.complete(925);
		expect(pager.next()?.window).toEqual({ index: 2, since: 900, until: 924 });
		pager.complete(900);
		expect(pager.next()?.window).toEqual({ index: 3, since: 800, until: 899 });
	});

	it('grows older windows for up to the configured empty attempts', () => {
		const pager = createTimeWindowPager({
			subId: 'sparse',
			requests,
			anchor: 1_000,
			windowSeconds: 100,
			maxEmptyPages: 3,
			emptyWindowGrowthFactor: 2
		});

		expect(pager.next()?.window).toEqual({ index: 0, since: 900, until: 999 });
		expect(pager.complete()).toEqual({
			hasMore: true,
			shouldRetry: true,
			consecutiveEmptyPages: 1
		});
		expect(pager.next()?.window).toEqual({ index: 1, since: 700, until: 899 });
		expect(pager.complete()).toEqual({
			hasMore: true,
			shouldRetry: true,
			consecutiveEmptyPages: 2
		});
		expect(pager.next()?.window).toEqual({ index: 2, since: 300, until: 699 });
		expect(pager.complete()).toEqual({
			hasMore: false,
			shouldRetry: false,
			consecutiveEmptyPages: 3
		});
		expect(pager.next()).toBeNull();
	});

	it('resets empty growth after receiving an event', () => {
		const pager = createTimeWindowPager({
			subId: 'sparse-then-dense',
			requests,
			anchor: 1_000,
			windowSeconds: 100,
			maxEmptyPages: 3
		});

		pager.next();
		pager.complete();
		expect(pager.next()?.window).toEqual({ index: 1, since: 700, until: 899 });
		expect(pager.complete(850).consecutiveEmptyPages).toBe(0);
		expect(pager.next()?.window).toEqual({ index: 2, since: 700, until: 849 });
		expect(pager.complete().consecutiveEmptyPages).toBe(1);
		expect(pager.next()?.window).toEqual({ index: 3, since: 500, until: 699 });
	});

	it('resets the page index and optionally moves the anchor', () => {
		vi.spyOn(Date, 'now').mockReturnValue(2_000_000);
		const pager = createTimeWindowPager({
			subId: 'feed',
			requests,
			anchor: 1_000,
			windowSeconds: 100
		});
		pager.next();
		pager.reset(2_000);
		expect(pager.next()?.window).toEqual({ index: 0, since: 1_900, until: 1_999 });
		vi.restoreAllMocks();
	});

	it('rejects invalid pager inputs', () => {
		expect(() => createTimeWindowPager({ subId: '', requests, windowSeconds: 10 })).toThrow(
			'subId is required'
		);
		expect(() => timeWindowForPage(1_000, 0, 0)).toThrow(
			'windowSeconds must be a positive integer'
		);
		expect(() => timeWindowForPage(1_000, 100, -1)).toThrow(
			'page index must be a non-negative integer'
		);
		const pager = createTimeWindowPager({
			subId: 'profile',
			requests,
			anchor: 1_000,
			windowSeconds: 100
		});
		expect(() => pager.complete()).toThrow('no active page to complete');
		pager.next();
		expect(() => pager.complete(899)).toThrow(
			'oldestReceivedAt must be inside the completed page window'
		);
	});
});
