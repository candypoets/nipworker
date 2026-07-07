import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

type Listener = () => void;

class MockWorker {
	static instances: MockWorker[] = [];
	public messages: any[] = [];
	public onmessage: ((event: MessageEvent) => void) | null = null;
	public readonly url: string;

	constructor(url: URL) {
		this.url = url.toString();
		MockWorker.instances.push(this);
	}

	postMessage(msg: any) {
		this.messages.push(msg);
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
		delete (globalThis as any).Worker;
		delete (globalThis as any).window;
		delete (globalThis as any).document;
		delete (globalThis as any).localStorage;
	});

	it('wakes the connections worker on pageshow, focus, and online foreground signals', async () => {
		const { NostrManager } = await import('./NostrManager');
		new NostrManager();
		const connections = MockWorker.instances.find((worker) => worker.url.includes('/connections/'));
		expect(connections).toBeDefined();

		windowTarget.dispatch('pageshow');
		expect(connections!.messages.at(-1)).toEqual({ type: 'wake', source: 'pageshow' });

		vi.setSystemTime(1_300);
		windowTarget.dispatch('focus');
		expect(connections!.messages.at(-1)).toEqual({ type: 'wake', source: 'focus' });

		vi.setSystemTime(1_600);
		windowTarget.dispatch('online');
		expect(connections!.messages.at(-1)).toEqual({ type: 'wake', source: 'online' });
	});

	it('still wakes on pageshow after visibilitychange already handled foregrounding', async () => {
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
		expect(connections!.messages.at(-1)).toEqual({ type: 'wake', source: 'visibility' });

		vi.setSystemTime(1_600);
		windowTarget.dispatch('pageshow');
		expect(connections!.messages.at(-1)).toEqual({ type: 'wake', source: 'pageshow' });
	});
});
