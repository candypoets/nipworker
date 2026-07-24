import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

type Listener = () => void;

class MockWorker {
	static instances: MockWorker[] = [];
	public messages: any[] = [];
	public onmessage: ((event: MessageEvent) => void) | null = null;
	public readonly url: string;
	public respondToPing = true;
	public terminated = false;
	private listeners = new Map<string, Set<EventListener>>();

	constructor(url: URL) {
		this.url = url.toString();
		MockWorker.instances.push(this);
	}

	postMessage(msg: any) {
		this.messages.push(msg);
		if (msg?.type === 'ping' && this.respondToPing) {
			const event = { data: { type: 'pong', id: msg.id } } as MessageEvent;
			for (const listener of this.listeners.get('message') || []) {
				listener(event);
			}
		}
	}

	addEventListener(type: string, listener: EventListener) {
		const listeners = this.listeners.get(type) || new Set<EventListener>();
		listeners.add(listener);
		this.listeners.set(type, listeners);
	}

	removeEventListener(type: string, listener: EventListener) {
		this.listeners.get(type)?.delete(listener);
	}

	terminate() {
		this.terminated = true;
	}
}

function createLifecycleTarget() {
	const listeners = new Map<string, Listener[]>();
	return {
		addEventListener(type: string, listener: Listener) {
			const arr = listeners.get(type) || [];
			arr.push(listener);
			listeners.set(type, arr);
		},
		dispatch(type: string) {
			for (const listener of listeners.get(type) || []) {
				listener();
			}
		}
	};
}

async function flushWakeCheck() {
	await Promise.resolve();
	await Promise.resolve();
}

describe('NostrManager lifecycle wake', () => {
	let windowTarget: ReturnType<typeof createLifecycleTarget>;
	let documentTarget: ReturnType<typeof createLifecycleTarget> & {
		hidden: boolean;
		visibilityState: 'hidden' | 'visible';
	};

	beforeEach(() => {
		vi.useFakeTimers();
		vi.setSystemTime(1_000);
		MockWorker.instances = [];
		windowTarget = createLifecycleTarget();
		documentTarget = Object.assign(createLifecycleTarget(), {
			hidden: false,
			visibilityState: 'visible' as const
		});
		(globalThis as any).Worker = MockWorker;
		(globalThis as any).window = windowTarget;
		(globalThis as any).document = documentTarget;
		(globalThis as any).localStorage = {
			getItem: vi.fn(),
			setItem: vi.fn(),
			removeItem: vi.fn()
		};
	});

	afterEach(() => {
		vi.useRealTimers();
		vi.restoreAllMocks();
		delete (globalThis as any).Worker;
		delete (globalThis as any).window;
		delete (globalThis as any).document;
		delete (globalThis as any).localStorage;
	});

	it('attempts wake on pageshow, focus, and online foreground signals', async () => {
		const { NostrManager } = await import('./NostrManager');
		new NostrManager();
		const connections = MockWorker.instances.find((worker) => worker.url.includes('/connections/'));
		expect(connections).toBeDefined();

		windowTarget.dispatch('pageshow');
		await flushWakeCheck();
		expect(connections!.messages.at(-1)).toEqual({ type: 'wake', source: 'pageshow' });

		vi.setSystemTime(11_000);
		windowTarget.dispatch('focus');
		await flushWakeCheck();
		expect(connections!.messages.at(-1)).toEqual({ type: 'wake', source: 'focus' });

		vi.setSystemTime(21_000);
		windowTarget.dispatch('online');
		await flushWakeCheck();
		expect(connections!.messages.at(-1)).toEqual({ type: 'wake', source: 'online' });
	});

	it('throttles accepted wake signals to one every ten seconds', async () => {
		const { NostrManager } = await import('./NostrManager');
		new NostrManager();
		const connections = MockWorker.instances.find((worker) => worker.url.includes('/connections/'));
		expect(connections).toBeDefined();

		windowTarget.dispatch('online');
		await flushWakeCheck();
		expect(connections!.messages.filter((message) => message?.type === 'wake')).toHaveLength(1);

		vi.setSystemTime(10_999);
		windowTarget.dispatch('online');
		await flushWakeCheck();
		expect(connections!.messages.filter((message) => message?.type === 'wake')).toHaveLength(1);

		vi.setSystemTime(11_000);
		windowTarget.dispatch('online');
		await flushWakeCheck();
		expect(connections!.messages.filter((message) => message?.type === 'wake')).toHaveLength(2);
	});

	it('uses focus as a fallback after an observed hidden transition', async () => {
		const { NostrManager } = await import('./NostrManager');
		new NostrManager();
		const connections = MockWorker.instances.find((worker) => worker.url.includes('/connections/'));
		expect(connections).toBeDefined();

		documentTarget.hidden = true;
		documentTarget.visibilityState = 'hidden';
		documentTarget.dispatch('visibilitychange');

		documentTarget.hidden = false;
		documentTarget.visibilityState = 'visible';
		vi.setSystemTime(1_300);
		windowTarget.dispatch('focus');
		await flushWakeCheck();
		expect(connections!.messages.at(-1)).toEqual({ type: 'wake', source: 'focus' });
	});

	it('throttles pageshow when it follows a handled visibility transition', async () => {
		const { NostrManager } = await import('./NostrManager');
		new NostrManager();
		const connections = MockWorker.instances.find((worker) => worker.url.includes('/connections/'));
		expect(connections).toBeDefined();

		documentTarget.hidden = true;
		documentTarget.visibilityState = 'hidden';
		documentTarget.dispatch('visibilitychange');

		documentTarget.hidden = false;
		documentTarget.visibilityState = 'visible';
		vi.setSystemTime(1_300);
		documentTarget.dispatch('visibilitychange');
		await flushWakeCheck();
		expect(connections!.messages.at(-1)).toEqual({ type: 'wake', source: 'visibility' });
		const wakeCount = connections!.messages.filter((message) => message?.type === 'wake').length;

		vi.setSystemTime(1_600);
		windowTarget.dispatch('pageshow');
		await flushWakeCheck();
		expect(connections!.messages.filter((message) => message?.type === 'wake')).toHaveLength(
			wakeCount
		);
	});

	it('rebuilds the entire worker graph when a worker does not answer on wake', async () => {
		const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
		const log = vi.spyOn(console, 'log').mockImplementation(() => {});
		const { NostrManager } = await import('./NostrManager');
		new NostrManager();
		const originalWorkers = [...MockWorker.instances];
		const cache = originalWorkers.find((worker) => worker.url.includes('/cache/'));
		expect(cache).toBeDefined();
		cache!.respondToPing = false;

		windowTarget.dispatch('pagehide');
		windowTarget.dispatch('pageshow');
		await vi.advanceTimersByTimeAsync(1_500);

		expect(originalWorkers.every((worker) => worker.terminated)).toBe(true);
		expect(MockWorker.instances).toHaveLength(8);
		expect(warn).toHaveBeenCalledWith(
			expect.stringContaining(
				'Restarting all workers (health check failed on pageshow); generation=1'
			)
		);
		expect(log).toHaveBeenCalledWith(
			'[main] Worker graph restart complete; generation=2, signer session restore scheduled'
		);
	});

	it('does not rebuild the worker graph if the app backgrounds during the health check', async () => {
		const { NostrManager } = await import('./NostrManager');
		new NostrManager();
		const cache = MockWorker.instances.find((worker) => worker.url.includes('/cache/'));
		expect(cache).toBeDefined();
		cache!.respondToPing = false;

		windowTarget.dispatch('online');
		documentTarget.hidden = true;
		documentTarget.visibilityState = 'hidden';
		documentTarget.dispatch('visibilitychange');
		await vi.advanceTimersByTimeAsync(1_500);

		expect(MockWorker.instances).toHaveLength(4);
		expect(MockWorker.instances.every((worker) => !worker.terminated)).toBe(true);
	});

	it('runs a fresh health check after foregrounding during an older probe', async () => {
		const { NostrManager } = await import('./NostrManager');
		new NostrManager();
		const cache = MockWorker.instances.find((worker) => worker.url.includes('/cache/'));
		expect(cache).toBeDefined();
		cache!.respondToPing = false;

		windowTarget.dispatch('online');
		documentTarget.hidden = true;
		documentTarget.visibilityState = 'hidden';
		documentTarget.dispatch('visibilitychange');
		documentTarget.hidden = false;
		documentTarget.visibilityState = 'visible';
		documentTarget.dispatch('visibilitychange');
		await vi.advanceTimersByTimeAsync(3_000);

		expect(MockWorker.instances).toHaveLength(8);
	});
});
