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
	Template,
	GetPublicKey,
	Subscribe,
	Publish,
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

	// Build tags
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

function sendCryptoResponse(json: string) {
	sendViaCallback('crypto', buildCryptoRawMessage(json));
}

function sendDirectResponse(workerBytes: Uint8Array) {
	// handleDirectResponse expects an extra 4-byte length prefix before the WorkerMessage
	const prefixed = new Uint8Array(4 + workerBytes.length);
	const view = new DataView(prefixed.buffer);
	view.setUint32(0, workerBytes.length, true);
	prefixed.set(workerBytes, 4);
	sendViaCallback('', prefixed);
}

// Set up the mock native module
(window as any).NativeModules = {
	NipworkerLynxModule: {
		init: function (cb: any) {
			(window as any).__nativeCallback = cb;
		},
		handleMessage: function (data: ArrayBuffer) {
			// Parse incoming MainMessage to determine the request type
			try {
				const bb = new flatbuffers.ByteBuffer(new Uint8Array(data));
				const mainMsg = MainMessage.getRootAsMainMessage(bb);
				const contentType = mainMsg.contentType();

				if (contentType === MainContent.GetPublicKey) {
					// Respond with Pubkey
					sendDirectResponse(buildPubkeyMessage(TEST_PUBKEY));
				} else if (contentType === MainContent.SignEvent) {
					// Parse the template and echo back with a fake signature
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
				} else if (contentType === MainContent.Subscribe || contentType === MainContent.Publish) {
					// For subscriptions/publishes, just acknowledge
					sendDirectResponse(buildPubkeyMessage(TEST_PUBKEY));
				} else {
					// Default fallback
					sendDirectResponse(buildPubkeyMessage(TEST_PUBKEY));
				}
			} catch (e) {
				console.warn('[native-mock] Failed to parse message:', e);
				sendDirectResponse(buildPubkeyMessage(TEST_PUBKEY));
			}
		},
		setPrivateKey: function (_key: string) {
			sendCryptoResponse(
				JSON.stringify({ op: 'set_signer', result: TEST_PUBKEY })
			);
		},
		deinit: function () {},
	},
};
