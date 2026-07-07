import { beforeEach, describe, expect, it, vi } from 'vitest';
import * as flatbuffers from 'flatbuffers';

import { createNostrManager } from './react-native';
import { setManager } from './manager';
import { useSubscription } from './hooks';
import { Eoce, Message, MessageType, WorkerMessage } from './generated/nostr/fb';

let nativeEventListener: ((event: any) => void) | undefined;
let appStateListener: ((state: 'active' | 'background' | 'inactive') => void) | undefined;
const queuedBuffers: ArrayBuffer[] = [];
const nativeBuffers = new Map<string, ArrayBuffer>();

vi.mock('react-native', () => {
	const turboModule = {
		init: vi.fn(),
		handleMessage: vi.fn(),
		installByteRuntime: vi.fn(() => {
			(globalThis as any).__nipworkerReactNativeByteRuntime = {
				init: vi.fn(),
				handleMessage: vi.fn(),
				wake: vi.fn(),
				setPrivateKey: vi.fn(),
				deinit: vi.fn(),
				drain: vi.fn(() => queuedBuffers.splice(0)),
				subscribe: vi.fn((_bytes: ArrayBuffer, subId: string) => {
					const buffer = nativeBuffers.get(subId) ?? new ArrayBuffer(4096);
					if (!nativeBuffers.has(subId)) {
						new DataView(buffer).setUint32(0, 4, true);
						nativeBuffers.set(subId, buffer);
					}
					return buffer;
				}),
				publish: vi.fn((_bytes: ArrayBuffer, publishId: string) => {
					const buffer = nativeBuffers.get(publishId) ?? new ArrayBuffer(3072);
					if (!nativeBuffers.has(publishId)) {
						new DataView(buffer).setUint32(0, 4, true);
						nativeBuffers.set(publishId, buffer);
					}
					return buffer;
				}),
				registerSubscription: vi.fn((subId: string, bufferSize: number) => {
					const buffer = new ArrayBuffer(bufferSize);
					new DataView(buffer).setUint32(0, 4, true);
					nativeBuffers.set(subId, buffer);
					return true;
				}),
				registerPublishBuffer: vi.fn(() => true),
				retainSubscriptionBuffer: vi.fn((subId: string) => nativeBuffers.get(subId)),
				retainSubscription: vi.fn(() => true),
				releaseSubscription: vi.fn(),
				getSubscriptionBuffer: vi.fn((subId: string) => nativeBuffers.get(subId)),
				cleanupSubscriptions: vi.fn()
			};
			return true;
		}),
		setPrivateKey: vi.fn(),
		getStorageItem: vi.fn(() => null),
		setStorageItem: vi.fn(() => true),
		removeStorageItem: vi.fn(() => true),
		deinit: vi.fn()
	};

	class NativeEventEmitter {
		addListener(_eventName: string, listener: (event: any) => void) {
			nativeEventListener = listener;
			return {
				remove: vi.fn(() => {
					if (nativeEventListener === listener) nativeEventListener = undefined;
				})
			};
		}
	}

	return {
		AppState: {
			currentState: 'active',
			addEventListener: vi.fn(
				(_eventName: string, listener: (state: 'active' | 'background' | 'inactive') => void) => {
					appStateListener = listener;
					return {
						remove: vi.fn(() => {
							if (appStateListener === listener) appStateListener = undefined;
						})
					};
				}
			)
		},
		NativeModules: {},
		NativeEventEmitter,
		TurboModuleRegistry: {
			get: vi.fn(() => turboModule)
		}
	};
});

function buildEoceMessage(subId: string): Uint8Array {
	const builder = new flatbuffers.Builder(256);
	const subscriptionId = builder.createString(subId);
	const eoce = Eoce.createEoce(builder, subscriptionId);
	const message = WorkerMessage.createWorkerMessage(
		builder,
		0,
		0,
		MessageType.Eoce,
		Message.Eoce,
		eoce
	);
	builder.finish(message);
	return builder.asUint8Array();
}

function createSubscriptionBuffer(payload: Uint8Array): ArrayBuffer {
	const buffer = new ArrayBuffer(4 + 4 + payload.length);
	const view = new DataView(buffer);
	const bytes = new Uint8Array(buffer);
	view.setUint32(0, buffer.byteLength, true);
	view.setUint32(4, payload.length, true);
	bytes.set(payload, 8);
	return buffer;
}

function buildRouteWakeFrame(subId: string): ArrayBuffer {
	const subIdBytes = new TextEncoder().encode(subId);
	const frame = new Uint8Array(8 + subIdBytes.length);
	frame.set([0x4e, 0x57, 0x52, 0x31], 0);
	new DataView(frame.buffer).setUint32(4, subIdBytes.length, true);
	frame.set(subIdBytes, 8);
	return frame.buffer;
}

describe('react-native byte runtime subscription path', () => {
	beforeEach(() => {
		nativeEventListener = undefined;
		appStateListener = undefined;
		queuedBuffers.length = 0;
		nativeBuffers.clear();
		delete (globalThis as any).__nipworkerReactNativeByteRuntime;
	});

	it('drains queued ArrayBuffers and delivers parsed messages to useSubscription', async () => {
		const manager = createNostrManager();
		setManager(manager);

		const callback = vi.fn();
		const byteRuntime = (globalThis as any).__nipworkerReactNativeByteRuntime;
		byteRuntime.subscribe = vi.fn((_bytes: ArrayBuffer, subId: string) => {
			nativeBuffers.set(subId, createSubscriptionBuffer(buildEoceMessage(subId)));
			return nativeBuffers.get(subId);
		});
		const unsubscribe = useSubscription('turbo-sub', [{ kinds: [1], limit: 1 }], callback, {
			closeOnEose: true
		});

		queuedBuffers.push(buildRouteWakeFrame('turbo-sub'));
		nativeEventListener?.({ v: 1, encoding: 'queued' });
		await Promise.resolve();
		await Promise.resolve();

		expect(callback).toHaveBeenCalledTimes(1);
		const message = callback.mock.calls[0][0] as WorkerMessage;
		expect(message.type()).toBe(MessageType.Eoce);
		expect(message.content(new Eoce())?.subscriptionId()).toBe('turbo-sub');

		unsubscribe();
		manager.deinit();
	});

	it('reads subscription messages from a native-owned ArrayBuffer', async () => {
		const manager = createNostrManager();
		setManager(manager);
		const byteRuntime = (globalThis as any).__nipworkerReactNativeByteRuntime;
		byteRuntime.subscribe = vi.fn((_bytes: ArrayBuffer, subId: string) => {
			nativeBuffers.set(subId, createSubscriptionBuffer(buildEoceMessage(subId)));
			return nativeBuffers.get(subId);
		});
		byteRuntime.getSubscriptionBuffer = vi.fn((subId: string) => nativeBuffers.get(subId));
		byteRuntime.retainSubscription = vi.fn(() => true);
		byteRuntime.releaseSubscription = vi.fn();
		byteRuntime.cleanupSubscriptions = vi.fn();

		const callback = vi.fn();
		const unsubscribe = useSubscription('native-owned-sub', [{ kinds: [1], limit: 1 }], callback, {
			closeOnEose: true
		});

		queuedBuffers.push(buildRouteWakeFrame('native-owned-sub'));
		nativeEventListener?.({ v: 1, encoding: 'queued' });
		await Promise.resolve();
		await Promise.resolve();

		expect(byteRuntime.subscribe).toHaveBeenCalledWith(expect.any(ArrayBuffer), 'native-owned-sub');
		expect(callback).toHaveBeenCalledTimes(1);
		const message = callback.mock.calls[0][0] as WorkerMessage;
		expect(message.type()).toBe(MessageType.Eoce);
		expect(message.content(new Eoce())?.subscriptionId()).toBe('native-owned-sub');

		unsubscribe();
		expect(byteRuntime.releaseSubscription).toHaveBeenCalledWith('native-owned-sub');
		appStateListener?.('background');
		expect(byteRuntime.cleanupSubscriptions).toHaveBeenCalled();
		manager.deinit();
	});

	it('reuses an existing native subscription buffer without resubscribing', () => {
		const manager = createNostrManager();
		setManager(manager);
		const byteRuntime = (globalThis as any).__nipworkerReactNativeByteRuntime;
		const buffer = createSubscriptionBuffer(buildEoceMessage('shared-sub'));
		nativeBuffers.set('shared-sub', buffer);

		const first = manager.subscribe('shared-sub', [{ kinds: [1], limit: 1 }], {
			closeOnEose: false
		});
		const second = manager.subscribe('shared-sub', [{ kinds: [1], limit: 1 }], {
			closeOnEose: false
		});

		expect(first).toBe(buffer);
		expect(second).toBe(buffer);
		expect(byteRuntime.subscribe).not.toHaveBeenCalledWith(expect.any(ArrayBuffer), 'shared-sub');
		expect(byteRuntime.retainSubscriptionBuffer).toHaveBeenCalledTimes(2);

		manager.unsubscribe('shared-sub');
		manager.unsubscribe('shared-sub');
		manager.deinit();
	});

	it('runs cleanup when the app backgrounds', () => {
		const manager = createNostrManager();
		setManager(manager);

		const unsubscribe = useSubscription(
			'background-cleanup-sub',
			[{ kinds: [1], limit: 1 }],
			vi.fn(),
			{
				closeOnEose: true
			}
		);
		const byteRuntime = (globalThis as any).__nipworkerReactNativeByteRuntime;

		unsubscribe();
		appStateListener?.('background');

		expect(byteRuntime.releaseSubscription).toHaveBeenCalledWith('background-cleanup-sub');
		expect(byteRuntime.cleanupSubscriptions).toHaveBeenCalled();
		manager.deinit();
	});

	it('delegates publish buffer ownership to the byte runtime', () => {
		const manager = createNostrManager();
		setManager(manager);
		const byteRuntime = (globalThis as any).__nipworkerReactNativeByteRuntime;

		const buffer = manager.publish('publish-1', {
			id: '0'.repeat(64),
			pubkey: '0'.repeat(64),
			created_at: 1,
			kind: 1,
			tags: [],
			content: 'hello',
			sig: '0'.repeat(128)
		});

		expect(buffer).toBe(nativeBuffers.get('publish-1'));
		expect(byteRuntime.publish).toHaveBeenCalledWith(expect.any(ArrayBuffer), 'publish-1');
		expect(byteRuntime.registerPublishBuffer).not.toHaveBeenCalled();

		manager.releasePublish?.('publish-1');
		expect(byteRuntime.releaseSubscription).toHaveBeenCalledWith('publish-1');
		manager.deinit();
	});
});
