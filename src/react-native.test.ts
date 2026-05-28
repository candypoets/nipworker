import { beforeEach, describe, expect, it, vi } from 'vitest';
import * as flatbuffers from 'flatbuffers';

import { createNostrManager } from './react-native';
import { setManager } from './manager';
import { useSubscription } from './hooks';
import { Eoce, Message, MessageType, WorkerMessage } from './generated/nostr/fb';

let nativeEventListener: ((event: any) => void) | undefined;
const queuedBuffers: ArrayBuffer[] = [];

vi.mock('react-native', () => {
	const turboModule = {
		init: vi.fn(),
		handleMessage: vi.fn(),
		installByteRuntime: vi.fn(() => {
			(globalThis as any).__nipworkerReactNativeByteRuntime = {
				init: vi.fn(),
				handleMessage: vi.fn(),
				setPrivateKey: vi.fn(),
				deinit: vi.fn(),
				drain: vi.fn(() => queuedBuffers.splice(0))
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

function buildNativeFrame(subId: string, payload: Uint8Array): ArrayBuffer {
	const subIdBytes = new TextEncoder().encode(subId);
	const frame = new Uint8Array(4 + subIdBytes.length + 4 + payload.length);
	const view = new DataView(frame.buffer);
	let offset = 0;
	view.setUint32(offset, subIdBytes.length, true);
	offset += 4;
	frame.set(subIdBytes, offset);
	offset += subIdBytes.length;
	view.setUint32(offset, payload.length, true);
	offset += 4;
	frame.set(payload, offset);
	return frame.buffer;
}

describe('react-native byte runtime subscription path', () => {
	beforeEach(() => {
		nativeEventListener = undefined;
		queuedBuffers.length = 0;
		delete (globalThis as any).__nipworkerReactNativeByteRuntime;
	});

	it('drains queued ArrayBuffers and delivers parsed messages to useSubscription', async () => {
		const manager = createNostrManager();
		setManager(manager);

		const callback = vi.fn();
		const unsubscribe = useSubscription('turbo-sub', [{ kinds: [1], limit: 1 }], callback, {
			closeOnEose: true
		});

		queuedBuffers.push(buildNativeFrame('turbo-sub', buildEoceMessage('turbo-sub')));
		nativeEventListener?.({ v: 1, encoding: 'queued' });
		await Promise.resolve();
		await Promise.resolve();

		expect(callback).toHaveBeenCalledTimes(1);
		const message = callback.mock.calls[0][0] as WorkerMessage;
		expect(message.type()).toBe(MessageType.Eoce);
		expect(message.content(new Eoce())?.subscriptionId()).toBe('turbo-sub');

		unsubscribe();
	});
});
