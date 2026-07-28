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
// nostrconnect mode (QR login): watches a text file for a nostrconnect:// URL
// written by the app under test. When it changes, the signer "scans" it and
// sends the NIP-46 connect ack (kind 24133, p-tagged to the client pubkey,
// NIP-44 encrypted JSON-RPC response with result === the URL's secret). The ack
// is re-sent every 2s until the client proceeds (a kind-24133 event from the
// client pubkey arrives) or 60s elapse, to cover the race where the app's
// subscription is not open yet at file-change time.
//
// Delivery goes through publish(): embedded routeEvent() plus a broadcast to
// OUTBOUND relay connections. For every relay in the nostrconnect URL that is
// not this script's own embedded relay, a persistent WebSocket client
// connection is opened (once per unique URL, auto-reconnecting every ~3s),
// subscribed to kind-24133 events p-tagged to the signer, so the whole NIP-46
// flow also works over public relays (wss://...) with no local relay in the URL.
//
// Auth-challenge mode (NIP-46 auth_url): when MOCK_AUTH_URL is set, the signer
// answers a bunker `connect` request (and the initial nostrconnect handshake)
// with {id, result: "auth_url", error: MOCK_AUTH_URL} instead of the real
// response. The deferred real response (same id) is sent once the file at
// MOCK_APPROVE_FILE appears/changes ("user approval"). Other methods answer
// normally without a challenge. Only one challenged exchange is tracked at a
// time; a new challenge replaces the old one.
//
// Config: --port (default 7746) or MOCK_PORT env.
//         MOCK_NOSTRCONNECT_FILE env (default /tmp/nostrconnect-url.txt).
//         MOCK_AUTH_URL env (empty = auth-challenge mode off, default off).
//         MOCK_APPROVE_FILE env (default /tmp/nip46-approve).

import fs from 'node:fs';
import { randomBytes } from 'node:crypto';
import WebSocket, { WebSocketServer } from 'ws';
import { getPublicKey, finalizeEvent, matchFilter, nip04, nip44 } from 'nostr-tools';

function arg(name, fallback) {
	const i = process.argv.indexOf(`--${name}`);
	if (i !== -1 && process.argv[i + 1] !== undefined) return Number(process.argv[i + 1]);
	const env = process.env[`MOCK_${name.replace(/-/g, '_').toUpperCase()}`];
	return env !== undefined ? Number(env) : fallback;
}

const PORT = arg('port', 7746);

// Auth-challenge mode: empty = off.
const AUTH_URL = process.env.MOCK_AUTH_URL || '';
const APPROVE_FILE = process.env.MOCK_APPROVE_FILE || '/tmp/nip46-approve';

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

// --- outbound relay connections ----------------------------------------------
// Persistent ws CLIENT connections to the relays named in nostrconnect URLs,
// so the signer flow works without a local relay in the URL. One connection
// per unique relay URL, reused across URLs, reconnecting forever.
const outbound = new Map(); // Map<relayUrl, WebSocket>
let outboundCounter = 0;

// True if relayUrl points back at this script's own embedded relay server.
function isSelfRelay(relayUrl) {
	try {
		const u = new URL(relayUrl);
		const host = u.hostname.replace(/^\[|\]$/g, '');
		const port = Number(u.port || (u.protocol === 'wss:' ? 443 : 80));
		return ['localhost', '127.0.0.1', '10.0.2.2', '::1'].includes(host) && port === PORT;
	} catch {
		return false;
	}
}

function connectOutbound(relayUrl) {
	if (outbound.has(relayUrl)) return;
	let ws;
	try {
		ws = new WebSocket(relayUrl);
	} catch (e) {
		console.log(`[signer-relay] outbound ${relayUrl} error: ${e.message} (invalid URL, not retrying)`);
		return;
	}
	outbound.set(relayUrl, ws);
	const subId = `out-${++outboundCounter}`;
	ws.on('open', () => {
		console.log(`[signer-relay] outbound ${relayUrl}: connected, subscribing ${subId}`);
		ws.send(JSON.stringify(['REQ', subId, { kinds: [24133], '#p': [SIGNER_PK] }]));
	});
	ws.on('message', (data) => {
		let msg;
		try {
			msg = JSON.parse(data.toString());
		} catch {
			return;
		}
		if (!Array.isArray(msg) || msg[0] !== 'EVENT') return;
		const evt = msg[2];
		if (!evt || evt.kind !== 24133 || evt.pubkey === SIGNER_PK) return; // skip own echoes
		console.log(`[signer-relay] outbound ${relayUrl}: EVENT kind=24133 from=${evt.pubkey?.slice(0, 8)}`);
		handleInbound24133(evt);
	});
	ws.on('error', (e) => {
		console.log(`[signer-relay] outbound ${relayUrl} error: ${e.message}`);
	});
	ws.on('close', () => {
		if (outbound.get(relayUrl) !== ws) return; // superseded
		outbound.delete(relayUrl);
		console.log(`[signer-relay] outbound ${relayUrl} closed, retrying in 3s`);
		setTimeout(() => connectOutbound(relayUrl), 3000);
	});
}

// Deliver an event both to embedded subscribers and to every open outbound relay.
function publish(evt) {
	routeEvent(evt);
	const frame = JSON.stringify(['EVENT', evt]);
	for (const [relayUrl, ws] of outbound) {
		if (ws.readyState === WebSocket.OPEN) {
			console.log(`[signer-relay] outbound ${relayUrl}: publishing kind=${evt.kind} id=${evt.id.slice(0, 8)}`);
			ws.send(frame);
		}
	}
}

// Shared handling for kind-24133 events from any source (embedded or outbound):
// stop the ack resend loop when the pending client proceeds, and answer
// requests addressed to the signer pubkey.
function handleInbound24133(evt) {
	// While an auth challenge is awaiting approval, client events do NOT count
	// as "proceeded" — resends continue until the approval is sent or timeout.
	if (resend && evt.pubkey === resend.clientPubkey && !resend.challenged) {
		stopResend(`client ${evt.pubkey.slice(0, 8)} proceeded`);
	}
	if (evt.tags?.some((t) => t[0] === 'p' && t[1] === SIGNER_PK)) {
		handleSignerRequest(evt);
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

	// Auth-challenge mode: challenge `connect` instead of answering it. The
	// deferred real response goes out when the approve file is touched.
	if (AUTH_URL && rpc.method === 'connect') {
		const challenge = { id: rpc.id, result: 'auth_url', error: AUTH_URL };
		const challengeEvt = finalizeEvent(
			{
				kind: 24133,
				created_at: now(),
				tags: [['p', evt.pubkey]],
				content: encryptResponse(evt.pubkey, JSON.stringify(challenge), useNip44)
			},
			SIGNER_SK
		);
		pendingChallenge = { mode: 'bunker', id: rpc.id, clientPubkey: evt.pubkey, useNip44 };
		console.log(`[signer-relay] auth challenge sent (id=${rpc.id}, url=${AUTH_URL})`);
		publish(challengeEvt);
		return;
	}

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
	publish(responseEvt);
}

// --- nostrconnect mode (QR login) -------------------------------------------
// The app under test writes its nostrconnect:// URL into this file; the signer
// "scans" it and initiates the handshake with a connect ack.
const NOSTRCONNECT_FILE = process.env.MOCK_NOSTRCONNECT_FILE || '/tmp/nostrconnect-url.txt';

// Outstanding ack resend state: { clientPubkey, timer, startedAt, challenged? }.
let resend = null;

// Outstanding auth-challenged exchange (auth-challenge mode only):
//   bunker:       { mode: 'bunker', id, clientPubkey, useNip44 }
//   nostrconnect: { mode: 'nostrconnect', id, clientPubkey, secret }
// A new challenge replaces the old one.
let pendingChallenge = null;

function stopResend(reason) {
	if (resend) {
		clearInterval(resend.timer);
		console.log(`[signer-relay] nostrconnect: stopped resending ack (${reason})`);
		resend = null;
	}
}

function sendConnectAck(clientPubkey, secret, id = randomBytes(8).toString('hex')) {
	const response = {
		id,
		result: secret,
		error: null
	};
	return finalizeEvent(
		{
			kind: 24133,
			created_at: now(),
			tags: [['p', clientPubkey]],
			content: nip44.v2.encrypt(JSON.stringify(response), conversationKey(clientPubkey))
		},
		SIGNER_SK
	);
}

function handleNostrconnectUrl(text) {
	let url;
	try {
		url = new URL(text);
	} catch (e) {
		console.log(`[signer-relay] nostrconnect: invalid URL: ${e.message}`);
		return;
	}
	if (url.protocol !== 'nostrconnect:') {
		console.log(`[signer-relay] nostrconnect: not a nostrconnect URL: ${url.protocol}`);
		return;
	}
	const clientPubkey = url.host;
	const relays = url.searchParams.getAll('relay');
	const secret = url.searchParams.get('secret');
	const name = url.searchParams.get('name');
	console.log(
		`[signer-relay] nostrconnect: parsed client=${clientPubkey} ` +
			`relays=${JSON.stringify(relays)} name=${name} secret=${secret}`
	);
	if (!/^[0-9a-fA-F]{64}$/.test(clientPubkey) || !secret) {
		console.log('[signer-relay] nostrconnect: missing/invalid client pubkey or secret, ignoring');
		return;
	}

	// Open outbound connections to the relays the client subscribes on (skipping
	// our own embedded relay, which is served by routeEvent()).
	for (const relayUrl of relays) {
		if (isSelfRelay(relayUrl)) {
			console.log(`[signer-relay] nostrconnect: skipping self relay ${relayUrl}`);
		} else {
			connectOutbound(relayUrl);
		}
	}

	stopResend('superseded by new URL');
	const startedAt = Date.now();

	if (AUTH_URL) {
		// Auth-challenge mode: send an auth_url challenge (stable id, so the
		// deferred approval can reuse it) instead of the connect ack, and keep
		// resending the challenge until approval or the 60s timeout.
		const challengeId = randomBytes(8).toString('hex');
		const challenge = { id: challengeId, result: 'auth_url', error: AUTH_URL };
		const challengeEvt = finalizeEvent(
			{
				kind: 24133,
				created_at: now(),
				tags: [['p', clientPubkey]],
				content: nip44.v2.encrypt(JSON.stringify(challenge), conversationKey(clientPubkey))
			},
			SIGNER_SK
		);
		pendingChallenge = { mode: 'nostrconnect', id: challengeId, clientPubkey, secret };
		publish(challengeEvt);
		console.log(`[signer-relay] auth challenge sent (id=${challengeId}, url=${AUTH_URL})`);

		const timer = setInterval(() => {
			if (Date.now() - startedAt > 60_000) {
				stopResend('60s timeout');
				return;
			}
			console.log(`[signer-relay] nostrconnect: resending auth challenge to ${clientPubkey}`);
			publish(challengeEvt);
		}, 2000);
		resend = { clientPubkey, timer, startedAt, challenged: true };
		return;
	}

	const ackEvt = sendConnectAck(clientPubkey, secret);
	publish(ackEvt);
	console.log(`[signer-relay] nostrconnect: sent connect ack to ${clientPubkey} secret=${secret}`);

	// The app's subscription may not be open yet: keep re-sending the ack every
	// 2s until the client proceeds (kind-24133 event from it arrives) or 60s pass.
	const timer = setInterval(() => {
		if (Date.now() - startedAt > 60_000) {
			stopResend('60s timeout');
			return;
		}
		console.log(`[signer-relay] nostrconnect: resending connect ack to ${clientPubkey}`);
		publish(sendConnectAck(clientPubkey, secret));
	}, 2000);
	resend = { clientPubkey, timer, startedAt };
}

function startNostrconnectWatch() {
	let last = '';
	setInterval(() => {
		let text;
		try {
			text = fs.readFileSync(NOSTRCONNECT_FILE, 'utf8').trim();
		} catch {
			return; // file does not exist (yet)
		}
		if (!text || text === last) return;
		last = text;
		handleNostrconnectUrl(text);
	}, 500);
	console.log(`[signer-relay] nostrconnect: watching ${NOSTRCONNECT_FILE}`);
}

// --- auth-challenge approval -------------------------------------------------
// The approve file appearing/changing means "the user approved the auth_url":
// send the deferred real response for the pending challenged exchange, reusing
// the challenge's request id.
function sendApproval() {
	const pending = pendingChallenge;
	if (!pending) return;
	pendingChallenge = null;
	console.log(`[signer-relay] approval file detected, sending deferred response (id=${pending.id})`);

	if (pending.mode === 'bunker') {
		const response = { id: pending.id, result: 'ack', error: null };
		publish(
			finalizeEvent(
				{
					kind: 24133,
					created_at: now(),
					tags: [['p', pending.clientPubkey]],
					content: encryptResponse(pending.clientPubkey, JSON.stringify(response), pending.useNip44)
				},
				SIGNER_SK
			)
		);
	} else {
		// nostrconnect: real connect ack with the URL's secret; stop the
		// challenge resend loop.
		stopResend('approval sent');
		publish(sendConnectAck(pending.clientPubkey, pending.secret, pending.id));
	}
}

function startApproveWatch() {
	// Ignore pre-existing content at startup; fire on create/touch/modify.
	let lastMtime;
	try {
		lastMtime = fs.statSync(APPROVE_FILE).mtimeMs;
	} catch {
		lastMtime = undefined;
	}
	setInterval(() => {
		let mtime;
		try {
			mtime = fs.statSync(APPROVE_FILE).mtimeMs;
		} catch {
			lastMtime = undefined; // deleted: next create fires again
			return;
		}
		if (mtime === lastMtime) return;
		lastMtime = mtime;
		sendApproval();
	}, 500);
	console.log(`[signer-relay] auth: watching approve file ${APPROVE_FILE}`);
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
			if (evt.kind === 24133) {
				handleInbound24133(evt);
			}
		} else if (type === 'CLOSE') {
			subs.get(ws)?.delete(String(msg[1]));
		}
	});
});

startNostrconnectWatch();
if (AUTH_URL) startApproveWatch();
