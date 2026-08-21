import { beforeEach, describe, expect, it, vi } from 'vitest';
import * as flatbuffers from 'flatbuffers';

import { createNostrManager } from './react-native';
import { setManager } from './manager';
import { useSignEvent, useSubscription } from './hooks';
import {
	Eoce,
	MainMessage,
	Message,
	MessageType,
	NostrEventT,
	PubkeyT,
	SetSignerResponseT,
	SignedEventT,
	StringVecT,
	Subscribe,
	WorkerMessage,
	WorkerMessageT
} from './generated/nostr/fb';

let nativeWakeHandler: (() => void) | undefined;
let appStateListener: ((state: 'active' | 'background' | 'inactive') => void) | undefined;
const queuedBuffers: ArrayBuffer[] = [];
const queuedRoutes: string[] = [];
const nativeBuffers = new Map<string, ArrayBuffer>();
const { initEngine, installByteRuntime, startMesh, setMeshProfile, clearMeshProfile, nativeStorage } = vi.hoisted(
	() => ({
		initEngine: vi.fn(),
		installByteRuntime: vi.fn(),
		startMesh: vi.fn(() => true),
		setMeshProfile: vi.fn(() => true),
		clearMeshProfile: vi.fn(() => true),
		nativeStorage: new Map<string, string>()
	})
);

vi.mock('react-native', () => {
	const turboModule = {
		init: vi.fn(),
		initEngine,
		handleMessage: vi.fn(),
		installByteRuntime: installByteRuntime.mockImplementation(() => {
			(globalThis as any).__nipworkerReactNativeByteRuntime = {
				init: vi.fn(),
				handleMessage: vi.fn(),
				wake: vi.fn(),
				setPrivateKey: vi.fn(),
				clearSigner: vi.fn(),
				removeSigner: vi.fn(),
				deinit: vi.fn(),
				setWakeHandler: vi.fn((handler?: () => void) => {
					nativeWakeHandler = handler;
				}),
				drainPending: vi.fn(() => ({
					routes: queuedRoutes.splice(0),
					packets: queuedBuffers.splice(0)
				})),
				getDeliveryStats: vi.fn(() => ({})),
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
		startMesh,
		stopMesh: vi.fn(),
		setMeshProfile,
		clearMeshProfile,
		setPrivateKey: vi.fn(),
		clearSigner: vi.fn(),
		removeSigner: vi.fn(),
		getStorageItem: vi.fn((key: string) => nativeStorage.get(key) ?? null),
		setStorageItem: vi.fn((key: string, value: string) => {
			nativeStorage.set(key, value);
			return true;
		}),
		removeStorageItem: vi.fn((key: string) => {
			nativeStorage.delete(key);
			return true;
		}),
		deinitEngine: vi.fn()
	};

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

function buildSignedEventMessage(): ArrayBuffer {
	const builder = new flatbuffers.Builder(1024);
	const message = new WorkerMessageT(
		'',
		'',
		MessageType.SignedEvent,
		Message.SignedEvent,
		new SignedEventT(
			new NostrEventT(
				'a'.repeat(64),
				'b'.repeat(64),
				9734,
				'hello',
				[new StringVecT(['p', 'c'.repeat(64)])],
				123,
				'd'.repeat(128)
			)
		)
	);
	builder.finish(message.pack(builder));
	const payload = builder.asUint8Array();
	const framed = new Uint8Array(4 + payload.length);
	new DataView(framed.buffer).setUint32(0, payload.length, true);
	framed.set(payload, 4);
	return framed.buffer;
}

function buildTypedSignedEventMessage(subId = 'crypto', requestId = 1): Uint8Array {
	const builder = new flatbuffers.Builder(1024);
	const message = new WorkerMessageT(
		subId,
		'',
		MessageType.SignedEvent,
		Message.SignedEvent,
		new SignedEventT(
			new NostrEventT(
				'a'.repeat(64),
				'b'.repeat(64),
				9734,
				'hello',
				[new StringVecT(['p', 'c'.repeat(64)])],
				123,
				'd'.repeat(128)
			),
			requestId
		)
	);
	builder.finish(message.pack(builder));
	return builder.asUint8Array();
}

function buildPubkeyMessage(pubkey: string): Uint8Array {
	const builder = new flatbuffers.Builder(512);
	const message = new WorkerMessageT(
		'crypto',
		'',
		MessageType.Pubkey,
		Message.Pubkey,
		new PubkeyT(pubkey)
	);
	builder.finish(message.pack(builder));
	return builder.asUint8Array();
}

function buildSetSignerResponse(pubkey: string): Uint8Array {
	const builder = new flatbuffers.Builder(512);
	const message = new WorkerMessageT(
		'crypto',
		'',
		MessageType.SetSignerResponse,
		Message.SetSignerResponse,
		new SetSignerResponseT(pubkey)
	);
	builder.finish(message.pack(builder));
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

describe('react-native byte runtime subscription path', () => {
	beforeEach(() => {
		nativeWakeHandler = undefined;
		appStateListener = undefined;
		queuedBuffers.length = 0;
		queuedRoutes.length = 0;
		nativeBuffers.clear();
		nativeStorage.clear();
		initEngine.mockClear();
		installByteRuntime.mockClear();
		startMesh.mockClear();
		setMeshProfile.mockClear();
		clearMeshProfile.mockClear();
		delete (globalThis as any).__nipworkerReactNativeByteRuntime;
	});

	it('installs the shared byte runtime before starting the engine', () => {
		const manager = createNostrManager({
			defaultRelays: ['wss://default.example'],
			indexerRelays: ['wss://indexer.example'],
			meshBLEEnabled: true,
			logLevel: 'info'
		});

		expect(initEngine).toHaveBeenCalledWith(
			['wss://default.example'],
			['wss://indexer.example'],
			true,
			'info'
		);
		expect(installByteRuntime.mock.invocationCallOrder[0]).toBeLessThan(
			initEngine.mock.invocationCallOrder[0]
		);
		expect(startMesh).toHaveBeenCalled();
		manager.deinit();
	});

	it('defaults the native log level to warn', () => {
		const manager = createNostrManager();

		expect(initEngine).toHaveBeenCalledWith([], [], false, 'warn');
		manager.deinit();
	});

	it('detaches the runtime-scoped wake handler before native deinit', () => {
		const manager = createNostrManager();
		const byteRuntime = (globalThis as any).__nipworkerReactNativeByteRuntime;
		const installedHandler = nativeWakeHandler;

		expect(installedHandler).toBeTypeOf('function');
		manager.deinit();

		expect(byteRuntime.setWakeHandler).toHaveBeenLastCalledWith(undefined);
		expect(nativeWakeHandler).toBeUndefined();
		queuedRoutes.push('late-after-deinit');
		expect(() => installedHandler?.()).not.toThrow();
		expect(queuedRoutes).toEqual(['late-after-deinit']);
	});

	it('configures and clears the visible mesh profile independently of BLE', () => {
		const manager = createNostrManager();
		const profile = {
			id: 'a'.repeat(64),
			pubkey: 'b'.repeat(64),
			created_at: 123,
			kind: 0,
			tags: [],
			content: '{"name":"Nearby"}',
			sig: 'c'.repeat(128)
		};

		expect(manager.setMeshProfile(profile)).toBe(true);
		expect(setMeshProfile).toHaveBeenCalledWith(JSON.stringify(profile));
		expect(manager.clearMeshProfile()).toBe(true);
		expect(clearMeshProfile).toHaveBeenCalled();
		manager.deinit();
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

		queuedRoutes.push('turbo-sub');
		nativeWakeHandler?.();
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

		queuedRoutes.push('native-owned-sub');
		nativeWakeHandler?.();
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

	it('serializes cache policy per request and keeps retired config fields neutral', () => {
		const manager = createNostrManager();
		const byteRuntime = (globalThis as any).__nipworkerReactNativeByteRuntime;

		manager.subscribe(
			'cache-first-defaults',
			[
				{ kinds: [1], maxRelays: 2 },
				{ kinds: [1], cacheFirst: true }
			],
			{ closeOnEose: false }
		);

		const serialized = byteRuntime.subscribe.mock.calls[0][0] as ArrayBuffer;
		const main = MainMessage.getRootAsMainMessage(
			new flatbuffers.ByteBuffer(new Uint8Array(serialized))
		);
		const subscribe = main.content(new Subscribe());

		expect(subscribe?.requests(0)?.cacheFirst()).toBe(false);
		expect(subscribe?.requests(0)?.maxRelays()).toBe(2);
		expect(subscribe?.requests(1)?.cacheFirst()).toBe(true);
		// Retained only for wire compatibility; no longer part of SubscriptionConfig.
		expect(subscribe?.config()?.cacheFirst()).toBe(true);
		expect(subscribe?.config()?.skipCache()).toBe(false);
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

	it('delivers direct signed-event responses to useSignEvent', () => {
		const manager = createNostrManager();
		setManager(manager);
		const callback = vi.fn();

		useSignEvent(
			{ kind: 9734, created_at: 123, content: 'hello', tags: [['p', 'c'.repeat(64)]] },
			callback
		);
		(manager as any).handleDirectResponse(new Uint8Array(buildSignedEventMessage()));

		expect(callback).toHaveBeenCalledWith({
			id: 'a'.repeat(64),
			pubkey: 'b'.repeat(64),
			created_at: 123,
			kind: 9734,
			tags: [['p', 'c'.repeat(64)]],
			content: 'hello',
			sig: 'd'.repeat(128)
		});
		manager.deinit();
	});

	it('routes typed signed-event responses to useSignEvent', () => {
		const manager = createNostrManager();
		setManager(manager);
		const callback = vi.fn();

		useSignEvent(
			{ kind: 9734, created_at: 123, content: 'hello', tags: [['p', 'c'.repeat(64)]] },
			callback
		);
		(manager as any).handleNativePayload(buildTypedSignedEventMessage());

		expect(callback).toHaveBeenCalledWith(
			expect.objectContaining({ kind: 9734, id: 'a'.repeat(64), sig: 'd'.repeat(128) })
		);
		manager.deinit();
	});

	it('delivers the unframed empty-sub-id SignedEvent emitted by native Rust', () => {
		const manager = createNostrManager();
		setManager(manager);
		const callback = vi.fn();
		const response = buildTypedSignedEventMessage('');

		useSignEvent(
			{ kind: 9734, created_at: 123, content: 'hello', tags: [['p', 'c'.repeat(64)]] },
			callback
		);
		queuedBuffers.push(response.slice().buffer);
		nativeWakeHandler?.();

		expect(callback).toHaveBeenCalledWith({
			id: 'a'.repeat(64),
			pubkey: 'b'.repeat(64),
			created_at: 123,
			kind: 9734,
			tags: [['p', 'c'.repeat(64)]],
			content: 'hello',
			sig: 'd'.repeat(128)
		});
		manager.deinit();
	});

	it('logout clears the native signer and ignores late auth and sign responses', async () => {
		const manager = createNostrManager();
		await Promise.resolve();
		const byteRuntime = (globalThis as any).__nipworkerReactNativeByteRuntime;
		const callback = vi.fn();
		const pubkey = 'b'.repeat(64);

		manager.setSigner('privkey', '1'.repeat(64));
		manager.signEvent({ kind: 1, created_at: 123, content: 'pending', tags: [] }, callback);
		manager.logout();

		expect(byteRuntime.clearSigner).toHaveBeenCalledTimes(1);
		expect(manager.getActivePubkey()).toBeNull();
		expect(nativeStorage.has('nostr_active_pubkey')).toBe(false);

		(manager as any).handleNativePayload(buildPubkeyMessage(pubkey));
		(manager as any).handleNativePayload(buildSetSignerResponse(pubkey));
		(manager as any).handleNativePayload(buildTypedSignedEventMessage('crypto', 1));

		expect(manager.getActivePubkey()).toBeNull();
		expect(callback).not.toHaveBeenCalled();
		manager.deinit();
	});

	it('retains saved accounts on logout and explicitly restores one on unlock', async () => {
		const manager = createNostrManager();
		await Promise.resolve();
		const byteRuntime = (globalThis as any).__nipworkerReactNativeByteRuntime;
		const secret = '2'.repeat(64);
		const pubkey = 'c'.repeat(64);

		manager.setSigner('privkey', secret);
		(manager as any).handleNativePayload(buildPubkeyMessage(pubkey));
		expect(manager.getAccounts()[pubkey]).toEqual({ type: 'privkey', payload: secret });

		manager.logout();
		expect(manager.getAccounts()[pubkey]).toEqual({ type: 'privkey', payload: secret });
		expect(nativeStorage.has('nostr_active_pubkey')).toBe(false);

		manager.switchAccount(pubkey);
		expect(byteRuntime.setPrivateKey).toHaveBeenCalledTimes(2);
		manager.deinit();
	});

	it('removeAccount forgets the active persisted credential and clears native state', async () => {
		const manager = createNostrManager();
		await Promise.resolve();
		const byteRuntime = (globalThis as any).__nipworkerReactNativeByteRuntime;
		const pubkey = 'd'.repeat(64);

		manager.setSigner('privkey', '3'.repeat(64));
		(manager as any).handleNativePayload(buildPubkeyMessage(pubkey));
		manager.removeAccount();

		expect(manager.getAccounts()).toEqual({});
		expect(manager.getActivePubkey()).toBeNull();
		expect(byteRuntime.clearSigner).toHaveBeenCalledTimes(1);
		manager.deinit();
	});

	it('removeAccount sends a remote logout before forgetting a NIP-46 credential', async () => {
		const manager = createNostrManager();
		await Promise.resolve();
		const byteRuntime = (globalThis as any).__nipworkerReactNativeByteRuntime;
		const pubkey = 'e'.repeat(64);

		manager.setSigner('nip46', {
			url: `bunker://${'f'.repeat(64)}?relay=wss%3A%2F%2Frelay.example`,
			clientSecret: '4'.repeat(64)
		});
		(manager as any).handleNativePayload(buildSetSignerResponse(pubkey));
		manager.removeAccount();

		expect(manager.getAccounts()).toEqual({});
		expect(manager.getActivePubkey()).toBeNull();
		expect(byteRuntime.removeSigner).toHaveBeenCalledTimes(1);
		expect(byteRuntime.clearSigner).toHaveBeenCalledTimes(1);
		manager.deinit();
	});

	it('never routes an unknown response id to another pending sign request', () => {
		const manager = createNostrManager();
		const first = vi.fn();
		const second = vi.fn();

		manager.signEvent({ kind: 1, created_at: 1, content: 'first', tags: [] }, first);
		manager.signEvent({ kind: 1, created_at: 2, content: 'second', tags: [] }, second);
		(manager as any).handleNativePayload(buildTypedSignedEventMessage('crypto', 999));

		expect(first).not.toHaveBeenCalled();
		expect(second).not.toHaveBeenCalled();

		(manager as any).handleNativePayload(buildTypedSignedEventMessage('crypto', 2));
		expect(first).not.toHaveBeenCalled();
		expect(second).toHaveBeenCalledTimes(1);
		manager.deinit();
	});
});
