#!/usr/bin/env node
// Mock Nostr relay with an embedded NIP-46 remote signer, for e2e tests.
//
// Relay side (just enough NIP-01):
//   ["REQ", subId, filter]  -> remember sub, send ["EOSE", subId]
//   ["EVENT", evt]          -> ["OK", id, true, ""], route to matching subs
//   ["CLOSE", subId]        -> drop the sub
//
// Signer side: holds a fixed keypair and answers kind-24133 JSON-RPC requests
// p-tagged to it (connect / get_public_key / sign_event / ping / nip04+44
// encrypt+decrypt). Requests are decrypted with NIP-44 first, NIP-04 fallback;
// responses use the same scheme as the request.
//
// The signer secret key is fixed so tests can hardcode the bunker URL:
//   bunker://<signerPubkey>?relay=ws://localhost:<port>
//
// Config: --port (default 7746) or MOCK_PORT env.

import { WebSocketServer } from 'ws';
import { getPublicKey, finalizeEvent, matchFilter, nip04, nip44 } from 'nostr-tools';

function arg(name, fallback) {
	const i = process.argv.indexOf(`--${name}`);
	if (i !== -1 && process.argv[i + 1] !== undefined) return Number(process.argv[i + 1]);
	const env = process.env[`MOCK_${name.replace(/-/g, '_').toUpperCase()}`];
	return env !== undefined ? Number(env) : fallback;
}

const PORT = arg('port', 7746);

// Fixed signer keypair (do NOT use outside tests).
const SIGNER_SK_HEX = 'aa'.repeat(32);
const SIGNER_SK = Uint8Array.from(Buffer.from(SIGNER_SK_HEX, 'hex'));
const SIGNER_PK = getPublicKey(SIGNER_SK);

// subs: Map<WebSocket, Map<subId, filter>>
const subs = new Map();

function send(ws, frame) {
	if (ws.readyState === ws.OPEN) ws.send(JSON.stringify(frame));
}

function routeEvent(evt) {
	for (const [ws, wsSubs] of subs) {
		for (const [subId, filter] of wsSubs) {
			if (matchFilter(filter, evt)) {
				console.log(`[signer-relay] routing kind=${evt.kind} to sub ${subId}`);
				send(ws, ['EVENT', subId, evt]);
			}
		}
	}
}

function now() {
	return Math.floor(Date.now() / 1000);
}

function conversationKey(pubkey) {
	return nip44.v2.utils.getConversationKey(SIGNER_SK, pubkey);
}

function decryptRequest(evt) {
	try {
		return { plaintext: nip44.v2.decrypt(evt.content, conversationKey(evt.pubkey)), nip44: true };
	} catch {
		return { plaintext: nip04.decrypt(SIGNER_SK, evt.pubkey, evt.content), nip44: false };
	}
}

function encryptResponse(clientPubkey, payload, useNip44) {
	return useNip44
		? nip44.v2.encrypt(payload, conversationKey(clientPubkey))
		: nip04.encrypt(SIGNER_SK, clientPubkey, payload);
}

function handleRpc(method, params) {
	switch (method) {
		case 'connect':
			return 'ack';
		case 'get_public_key':
			return SIGNER_PK;
		case 'ping':
			return 'pong';
		case 'sign_event': {
			const template = JSON.parse(params[0]);
			const signed = finalizeEvent(
				{
					kind: template.kind,
					created_at: template.created_at ?? now(),
					tags: template.tags ?? [],
					content: template.content ?? ''
				},
				SIGNER_SK
			);
			return JSON.stringify(signed);
		}
		case 'nip04_encrypt':
			return nip04.encrypt(SIGNER_SK, params[0], params[1]);
		case 'nip04_decrypt':
			return nip04.decrypt(SIGNER_SK, params[0], params[1]);
		case 'nip44_encrypt':
			return nip44.v2.encrypt(params[1], conversationKey(params[0]));
		case 'nip44_decrypt':
			return nip44.v2.decrypt(params[1], conversationKey(params[0]));
		default:
			throw new Error(`unsupported method: ${method}`);
	}
}

async function handleSignerRequest(evt) {
	let rpc;
	let useNip44;
	try {
		const { plaintext, nip44: is44 } = decryptRequest(evt);
		useNip44 = is44;
		rpc = JSON.parse(plaintext);
	} catch (e) {
		console.log(`[signer-relay] failed to decrypt/parse request: ${e.message}`);
		return;
	}

	console.log(`[signer-relay] rpc ${rpc.method} (id=${rpc.id}, nip44=${useNip44})`);

	let response;
	try {
		response = { id: rpc.id, result: handleRpc(rpc.method, rpc.params ?? []), error: null };
	} catch (e) {
		response = { id: rpc.id, result: null, error: e.message };
	}

	const responseEvt = finalizeEvent(
		{
			kind: 24133,
			created_at: now(),
			tags: [['p', evt.pubkey]],
			content: encryptResponse(evt.pubkey, JSON.stringify(response), useNip44)
		},
		SIGNER_SK
	);
	routeEvent(responseEvt);
}

const wss = new WebSocketServer({ port: PORT });

wss.on('listening', () => {
	console.log(`[signer-relay] listening on ws://localhost:${PORT}`);
	console.log(`[signer-relay] bunker=bunker://${SIGNER_PK}?relay=ws%3A%2F%2Flocalhost%3A${PORT}`);
});

wss.on('connection', (ws) => {
	console.log('[signer-relay] client connected');
	subs.set(ws, new Map());
	ws.on('close', () => subs.delete(ws));
	ws.on('message', (data) => {
		let msg;
		try {
			msg = JSON.parse(data.toString());
		} catch {
			return;
		}
		if (!Array.isArray(msg)) return;
		const type = msg[0];
		if (type === 'REQ') {
			const subId = String(msg[1]);
			const filter = msg.slice(2).find((f) => f && typeof f === 'object') || {};
			console.log(`[signer-relay] REQ ${subId} ${JSON.stringify(filter)}`);
			subs.get(ws)?.set(subId, filter);
			send(ws, ['EOSE', subId]);
		} else if (type === 'EVENT') {
			const evt = msg[1];
			if (!evt || !evt.id) return;
			console.log(`[signer-relay] EVENT kind=${evt.kind} from=${evt.pubkey?.slice(0, 8)}`);
			send(ws, ['OK', evt.id, true, '']);
			routeEvent(evt);
			if (evt.kind === 24133 && evt.tags?.some((t) => t[0] === 'p' && t[1] === SIGNER_PK)) {
				handleSignerRequest(evt);
			}
		} else if (type === 'CLOSE') {
			subs.get(ws)?.delete(String(msg[1]));
		}
	});
});
