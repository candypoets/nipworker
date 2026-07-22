#!/usr/bin/env node
// Minimal mock Nostr relay for the browser benchmark suite.
//
// Speaks just enough NIP-01:
//   ["REQ", subId, filter...]  -> N synthetic ["EVENT", subId, {...}] frames, then ["EOSE", subId]
//   ["EVENT", {...}] (publish) -> ["OK", id, true, ""]
//   ["CLOSE", subId]           -> silently drop the subscription
//
// Event count per REQ comes from the filter's `limit` (default 1000, capped).
// After EOSE, a burst of live events is sent if configured (LIVE > 0). Live
// events carry a ["bt", <unix-ms>] tag so the client can measure one-way latency.
//
// Config via CLI args (--port 7710) or env (MOCK_PORT):
//   port          (default 7710)
//   live          live events sent after each EOSE        (default 0)
//   live-interval ms between live events, 0 = full burst  (default 0)
//   live-delay    ms to wait after EOSE before live events(default 0)
//   live-prefix   only burst for subIds with this prefix  (default: all)
//   content-size  bytes of event content                  (default 280)
//   seed          PRNG seed                               (default 42)
//   max           cap on events per REQ                   (default 50000)
//   seed-by       'sub' (default, per-subscription stream) or 'filter':
//                 the served set depends only on kinds+limit, not the subId,
//                 so N relay instances serve the same events for the same
//                 filter (multi-relay overlap testing). created_at is also
//                 fixed in filter mode so streams are deterministic.
//   unique-fraction  0..1 (default 0, filter mode only): the tail fraction of
//                 the stream is seeded per PORT, so each relay serves
//                 ~fraction events no other relay has (realistic overlap).

import { WebSocketServer } from 'ws';

function arg(name, fallback) {
	const i = process.argv.indexOf(`--${name}`);
	if (i !== -1 && process.argv[i + 1] !== undefined) return Number(process.argv[i + 1]);
	const env = process.env[`MOCK_${name.replace(/-/g, '_').toUpperCase()}`];
	return env !== undefined ? Number(env) : fallback;
}

const PORT = arg('port', 7710);
const LIVE = arg('live', 0);
const LIVE_INTERVAL = arg('live-interval', 0);
const LIVE_DELAY = arg('live-delay', 0);
const LIVE_PREFIX = (() => {
	const i = process.argv.indexOf('--live-prefix');
	if (i !== -1 && process.argv[i + 1] !== undefined) return process.argv[i + 1];
	return process.env.MOCK_LIVE_PREFIX ?? '';
})();
const CONTENT_SIZE = arg('content-size', 280);
const SEED = arg('seed', 42);
const MAX = arg('max', 50000);
const SEED_BY = (() => {
	const i = process.argv.indexOf('--seed-by');
	if (i !== -1 && process.argv[i + 1] !== undefined) return process.argv[i + 1];
	return process.env.MOCK_SEED_BY ?? 'sub';
})();
const UNIQUE_FRACTION = Math.min(Math.max(arg('unique-fraction', 0), 0), 1);
// In filter-seed mode, pin the clock so every relay instance produces
// identical created_at values regardless of when it was started.
const FIXED_NOW = SEED_BY === 'filter' ? 1750000000 : 0;

// mulberry32 seeded PRNG
function mulberry32(seed) {
	let a = seed >>> 0;
	return function () {
		a |= 0;
		a = (a + 0x6d2b79f5) | 0;
		let t = Math.imul(a ^ (a >>> 15), 1 | a);
		t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
		return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
	};
}

function hashStr(s) {
	let h = 2166136261;
	for (let i = 0; i < s.length; i++) {
		h ^= s.charCodeAt(i);
		h = Math.imul(h, 16777619);
	}
	return h >>> 0;
}

function hex(rand, len) {
	const chars = '0123456789abcdef';
	let out = '';
	for (let i = 0; i < len; i++) out += chars[(rand() * 16) | 0];
	return out;
}

const CONTENT_CHARS = 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 ';

// Kind mix: mostly short text notes.
const KIND_MIX = [1, 1, 1, 1, 1, 1, 1, 1, 6, 7];

function pickKind(rand, filter) {
	if (filter && Array.isArray(filter.kinds) && filter.kinds.length > 0) {
		// Honor the requested kinds, weighted toward the first.
		const ks = filter.kinds;
		return rand() < 0.85 ? ks[0] : ks[(rand() * ks.length) | 0];
	}
	return KIND_MIX[(rand() * KIND_MIX.length) | 0];
}

function makeEvent(rand, filter, opts) {
	const pubkey = AUTHORS[(rand() * AUTHORS.length) | 0];
	const now = opts.live ? Math.floor(Date.now() / 1000) : FIXED_NOW || Math.floor(Date.now() / 1000);
	const createdAt = opts.live ? now : now - 60 - ((rand() * 3600) | 0);
	let content = '';
	for (let i = 0; i < CONTENT_SIZE; i++) {
		content += CONTENT_CHARS[(rand() * CONTENT_CHARS.length) | 0];
	}
	const tags = [];
	if (rand() < 0.3) tags.push(['p', hex(rand, 64)]);
	if (rand() < 0.2) tags.push(['t', `bench${(rand() * 100) | 0}`]);
	if (opts.live) tags.push(['bt', String(Date.now())]);
	return {
		id: hex(rand, 64),
		pubkey,
		created_at: createdAt,
		kind: pickKind(rand, filter),
		tags,
		content: `${content} #b${opts.index}`,
		sig: hex(rand, 128)
	};
}

// Fixed pool of authors (deterministic from SEED) so author filters can match.
const AUTHORS = (() => {
	const rand = mulberry32(SEED);
	const out = [];
	for (let i = 0; i < 50; i++) out.push(hex(rand, 64));
	return out;
})();

function sendCached(ws, subId, filter, count) {
	// Per-subscription seed (default): deterministic per subId, distinct across
	// subs (avoids cross-run id collisions with the client's dedup set).
	// Filter mode: the stream depends only on kinds+limit, so every relay
	// instance serves the same overlapping set for the same filter. The last
	// UNIQUE_FRACTION of the stream is re-seeded per PORT, giving each relay a
	// deterministic unique tail (realistic partial overlap).
	const filterKey = `k:${(filter.kinds ?? []).join(',')}|l:${count}`;
	const filterMode = SEED_BY === 'filter';
	const rand = mulberry32(filterMode ? SEED ^ hashStr(filterKey) : SEED ^ hashStr(subId));
	const tailStart = filterMode ? count - Math.round(count * UNIQUE_FRACTION) : count;
	let randTail = null;
	for (let i = 0; i < count; i++) {
		let r = rand;
		if (i >= tailStart) {
			if (!randTail) {
				randTail = mulberry32((SEED ^ hashStr(filterKey) ^ Math.imul(PORT, 2654435761)) >>> 0);
			}
			r = randTail;
		}
		ws.send(JSON.stringify(['EVENT', subId, makeEvent(r, filter, { index: i })]));
	}
	ws.send(JSON.stringify(['EOSE', subId]));
	return rand;
}

function sendLive(ws, subId, filter, rand) {
	const burst = () => {
		if (LIVE_INTERVAL <= 0) {
			for (let i = 0; i < LIVE; i++) {
				if (ws.readyState !== ws.OPEN) return;
				ws.send(JSON.stringify(['EVENT', subId, makeEvent(rand, filter, { index: `live${i}`, live: true })]));
			}
			return;
		}
		let i = 0;
		const tick = () => {
			if (i >= LIVE || ws.readyState !== ws.OPEN) return;
			ws.send(JSON.stringify(['EVENT', subId, makeEvent(rand, filter, { index: `live${i}`, live: true })]));
			i++;
			setTimeout(tick, LIVE_INTERVAL);
		};
		tick();
	};
	if (LIVE_DELAY > 0) setTimeout(burst, LIVE_DELAY);
	else burst();
}

const wss = new WebSocketServer({ port: PORT });

wss.on('listening', () => {
	console.log(`[mock-relay] listening on ws://localhost:${PORT} (live=${LIVE}, liveInterval=${LIVE_INTERVAL}ms, contentSize=${CONTENT_SIZE}, seed=${SEED}, seedBy=${SEED_BY}, uniqueFraction=${UNIQUE_FRACTION})`);
});

wss.on('connection', (ws) => {
	console.log('[mock-relay] client connected');
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
			const count = Math.min(Math.max(filter.limit ?? 1000, 0), MAX);
			console.log(`[mock-relay] REQ ${subId} limit=${count} kinds=${JSON.stringify(filter.kinds ?? null)}`);
			const rand = sendCached(ws, subId, filter, count);
			if (LIVE > 0 && (LIVE_PREFIX === '' || subId.startsWith(LIVE_PREFIX))) {
				sendLive(ws, subId, filter, rand);
			}
		} else if (type === 'EVENT') {
			const evt = msg[1];
			ws.send(JSON.stringify(['OK', evt && evt.id ? evt.id : '', true, '']));
		} else if (type === 'CLOSE') {
			// Nothing to clean up; subscriptions are stateless here.
		}
	});
});
