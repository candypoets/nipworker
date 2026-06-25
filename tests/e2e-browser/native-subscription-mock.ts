import * as flatbuffers from 'flatbuffers';
import {
	WorkerMessage,
	MessageType,
	Raw,
	Message,
	Pubkey,
	NostrEvent,
	SignedEvent,
	StringVec,
	MainMessage,
	MainContent,
	SignEvent,
	Subscribe,
	ParsedEvent,
	Eoce,
	ParsedData,
} from '../../src/generated/nostr/fb';

const TEST_PUBKEY = '79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798';

function buildCryptoRawMessage(json: string): Uint8Array {
	const builder = new flatbuffers.Builder(256);
	const rawStr = builder.createString(json);
	const raw = Raw.createRaw(builder, rawStr);
	const msg = WorkerMessage.createWorkerMessage(
		builder,
		0,
		0,
		MessageType.Raw,
		Message.Raw,
		raw
	);
	builder.finish(msg);
	return builder.asUint8Array();
}

function buildPubkeyMessage(pubkey: string): Uint8Array {
	const builder = new flatbuffers.Builder(256);
	const pubkeyStr = builder.createString(pubkey);
	const pubkeyObj = Pubkey.createPubkey(builder, pubkeyStr);
	const msg = WorkerMessage.createWorkerMessage(
		builder,
		0,
		0,
		MessageType.Pubkey,
		Message.Pubkey,
		pubkeyObj
	);
	builder.finish(msg);
	return builder.asUint8Array();
}

function buildSignedEventMessage(event: {
	id: string;
	pubkey: string;
	created_at: number;
	kind: number;
	tags: string[][];
	content: string;
	sig: string;
}): Uint8Array {
	const builder = new flatbuffers.Builder(512);

	const tagOffsets: number[] = [];
	for (const tag of event.tags) {
		const itemOffsets: number[] = [];
		for (const item of tag) {
			itemOffsets.push(builder.createString(item));
		}
		StringVec.startItemsVector(builder, itemOffsets.length);
		for (let i = itemOffsets.length - 1; i >= 0; i--) {
			builder.addOffset(itemOffsets[i]);
		}
		const itemsVec = builder.endVector();
		const tagObj = StringVec.createStringVec(builder, itemsVec);
		tagOffsets.push(tagObj);
	}

	NostrEvent.startTagsVector(builder, tagOffsets.length);
	for (let i = tagOffsets.length - 1; i >= 0; i--) {
		builder.addOffset(tagOffsets[i]);
	}
	const tagsVec = builder.endVector();

	const idStr = builder.createString(event.id);
	const pubkeyStr = builder.createString(event.pubkey);
	const contentStr = builder.createString(event.content);
	const sigStr = builder.createString(event.sig);

	const eventObj = NostrEvent.createNostrEvent(
		builder,
		idStr,
		pubkeyStr,
		event.kind,
		contentStr,
		tagsVec,
		event.created_at,
		sigStr
	);

	const signedEventObj = SignedEvent.createSignedEvent(builder, eventObj);

	const msg = WorkerMessage.createWorkerMessage(
		builder,
		0,
		0,
		MessageType.SignedEvent,
		Message.SignedEvent,
		signedEventObj
	);
	builder.finish(msg);
	return builder.asUint8Array();
}

function buildParsedEventMessage(event: {
	id: string;
	pubkey: string;
	kind: number;
	createdAt: number;
}): Uint8Array {
	const builder = new flatbuffers.Builder(256);
	const idStr = builder.createString(event.id);
	const pubkeyStr = builder.createString(event.pubkey);

	// Build empty tags vector (required field)
	ParsedEvent.startTagsVector(builder, 0);
	const tagsVec = builder.endVector();

	const parsedEventObj = ParsedEvent.createParsedEvent(
		builder,
		idStr,
		pubkeyStr,
		event.kind,
		event.createdAt,
		ParsedData.NONE,
		0,
		0,
		0,
		tagsVec
	);

	const msg = WorkerMessage.createWorkerMessage(
		builder,
		0,
		0,
		MessageType.ParsedNostrEvent,
		Message.ParsedEvent,
		parsedEventObj
	);
	builder.finish(msg);
	return builder.asUint8Array();
}

function buildEoceMessage(subscriptionId: string): Uint8Array {
	const builder = new flatbuffers.Builder(256);
	const subIdStr = builder.createString(subscriptionId);
	const eoceObj = Eoce.createEoce(builder, subIdStr);
	const msg = WorkerMessage.createWorkerMessage(
		builder,
		0,
		0,
		MessageType.Eoce,
		Message.Eoce,
		eoceObj
	);
	builder.finish(msg);
	return builder.asUint8Array();
}

function sendViaCallback(subId: string, workerBytes: Uint8Array) {
	const subIdBytes = new TextEncoder().encode(subId);
	const result = new Uint8Array(4 + subIdBytes.length + 4 + workerBytes.length);
	const view = new DataView(result.buffer);
	view.setUint32(0, subIdBytes.length, true);
	result.set(subIdBytes, 4);
	view.setUint32(4 + subIdBytes.length, workerBytes.length, true);
	result.set(workerBytes, 4 + subIdBytes.length + 4);
	setTimeout(() => {
		if ((window as any).__nativeCallback) {
			(window as any).__nativeCallback(result.buffer);
		}
	}, 50);
}

function sendDirectResponse(workerBytes: Uint8Array) {
	// handleDirectResponse expects an extra 4-byte length prefix before the WorkerMessage
	const prefixed = new Uint8Array(4 + workerBytes.length);
	const view = new DataView(prefixed.buffer);
	view.setUint32(0, workerBytes.length, true);
	prefixed.set(workerBytes, 4);
	sendViaCallback('', prefixed);
}

function sendSubscriptionResponse(subId: string, workerBytes: Uint8Array) {
	// Subscription responses need a 4-byte length prefix before WorkerMessage
	// because ArrayBufferReader.writeBatchedData expects length-prefixed data
	const prefixed = new Uint8Array(4 + workerBytes.length);
	const view = new DataView(prefixed.buffer);
	view.setUint32(0, workerBytes.length, true);
	prefixed.set(workerBytes, 4);
	sendViaCallback(subId, prefixed);
}

// Set up the mock native module
(window as any).NativeModules = {
	NipworkerLynxModule: {
		init: function (cb: any) {
			(window as any).__nativeCallback = cb;
		},
		handleMessage: function (data: ArrayBuffer) {
			try {
				const bb = new flatbuffers.ByteBuffer(new Uint8Array(data));
				const mainMsg = MainMessage.getRootAsMainMessage(bb);
				const contentType = mainMsg.contentType();

				if (contentType === MainContent.GetPublicKey) {
					sendDirectResponse(buildPubkeyMessage(TEST_PUBKEY));
				} else if (contentType === MainContent.SignEvent) {
					const signEventObj = mainMsg.content(new SignEvent());
					const templateObj = signEventObj ? signEventObj.template() : null;
					if (templateObj) {
						const tags: string[][] = [];
						for (let i = 0; i < templateObj.tagsLength(); i++) {
							const tag = templateObj.tags(i);
							if (tag) {
								const vals: string[] = [];
								for (let j = 0; j < tag.itemsLength(); j++) {
									const v = tag.items(j);
									if (v !== null) vals.push(v);
								}
								tags.push(vals);
							}
						}
						sendDirectResponse(
							buildSignedEventMessage({
								id: '0000000000000000000000000000000000000000000000000000000000000000',
								pubkey: TEST_PUBKEY,
								created_at: templateObj.createdAt(),
								kind: templateObj.kind(),
								tags,
								content: templateObj.content() || '',
								sig: '00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000',
							})
						);
					}
				} else if (contentType === MainContent.Subscribe) {
					// Parse subscription ID and send fake events
					const subscribeObj = mainMsg.content(new Subscribe());
					const subId = subscribeObj ? subscribeObj.subscriptionId() : 'unknown';

					// Send 3 fake events
					for (let i = 0; i < 3; i++) {
						setTimeout(() => {
							sendSubscriptionResponse(
								subId,
								buildParsedEventMessage({
									id: '000000000000000000000000000000000000000000000000000000000000000' + i,
									pubkey: TEST_PUBKEY,
									kind: 1,
									createdAt: Math.floor(Date.now() / 1000),
								})
							);
						}, 100 + i * 100);
					}

					// Send EOCE after events
					setTimeout(() => {
						sendSubscriptionResponse(subId, buildEoceMessage(subId));
					}, 500);
				} else if (contentType === MainContent.Publish) {
					sendDirectResponse(buildPubkeyMessage(TEST_PUBKEY));
				} else {
					sendDirectResponse(buildPubkeyMessage(TEST_PUBKEY));
				}
			} catch (e) {
				console.warn('[native-mock] Failed to parse message:', e);
				sendDirectResponse(buildPubkeyMessage(TEST_PUBKEY));
			}
		},
		setPrivateKey: function (_key: string) {
			const response = buildCryptoRawMessage(
				JSON.stringify({ op: 'set_signer', result: TEST_PUBKEY })
			);
			const subId = new TextEncoder().encode('crypto');
			const result = new Uint8Array(4 + subId.length + 4 + response.length);
			const view = new DataView(result.buffer);
			view.setUint32(0, subId.length, true);
			result.set(subId, 4);
			view.setUint32(4 + subId.length, response.length, true);
			result.set(response, 4 + subId.length + 4);
			setTimeout(() => {
				if ((window as any).__nativeCallback) {
					(window as any).__nativeCallback(result.buffer);
				}
			}, 50);
		},
		deinit: function () {},
	},
};
