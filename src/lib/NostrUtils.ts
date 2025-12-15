/**
 * Nostr utilities for PRE/RE list/set helpers on the frontend (TypeScript).
 *
 * This module provides:
 * - d-tag generation (slug or random)
 * - a-pointer computation (kind:pubkey_hex:d)
 * - naddr-like computation (human-readable, non-bech32 placeholder)
 *
 * Notes:
 * - naddr-like is NOT a real bech32-encoded naddr. It’s a convenient string to
 *   carry the same tuple ("kind:pubkey:d") plus optional relays for UI/links.
 * - If/when you wire a proper NIP-19 encoder, replace naddr-like with a true naddr.
 */

import { nip19 } from 'nostr-tools';

export type NostrTag = [string, ...string[]];

/* ---------------------------- NIP-19 naddr helpers --------------------------- */

/**
 * Build a NIP-19 naddr (bech32) from PRE tuple.
 * Relays are optional; pass when available to help clients.
 */
export function makeNaddr(
	kind: number,
	pubkeyHex: string,
	d: string,
	relays?: string[] | null
): string {
	if (!Number.isInteger(kind) || kind < 0 || kind > 0xffff) {
		throw new Error(`Invalid kind: ${kind}`);
	}
	if (!isHex64(pubkeyHex)) {
		throw new Error(`Invalid pubkey hex (expected 64 lowercase hex chars): ${pubkeyHex}`);
	}
	if (!d || typeof d !== 'string') {
		throw new Error(`Invalid d tag: ${d}`);
	}
	return nip19.naddrEncode({
		identifier: d,
		kind,
		pubkey: pubkeyHex,
		relays: relays && relays.length ? relays : undefined
	} as any);
}

/**
 * Build a NIP-19 naddr for an event. Requires that "d" is present (PRE).
 */
export function makeNaddrForEvent(evt: NostrEventMinimal, relays?: string[] | null): string {
	const d = extractDFromTags(evt.tags);
	if (!d) {
		throw new Error(`Missing "d" tag for PRE event of kind ${evt.kind}`);
	}
	return makeNaddr(evt.kind, evt.pubkey, d, relays);
}

/**
 * Parse a NIP-19 naddr into tuple and (optionally) relays.
 */
export function parseNaddr(naddr: string): {
	kind: number;
	pubkey: string;
	d: string;
	relays?: string[];
} {
	const decoded = nip19.decode(naddr);
	if (decoded.type !== 'naddr') {
		throw new Error(`Invalid naddr type: ${decoded.type}`);
	}
	const { identifier, kind, pubkey, relays } = decoded.data as any;
	return {
		kind,
		pubkey,
		d: identifier,
		relays
	};
}

export interface NostrEventMinimal {
	kind: number;
	pubkey: string; // 64-char hex
	tags: NostrTag[];
}

export interface APointer {
	kind: number;
	pubkey: string; // hex
	d: string;
}

/* -------------------------------- Utilities -------------------------------- */

export function isHex64(s: string): boolean {
	return /^[0-9a-f]{64}$/.test(s);
}

/**
 * Normalize a string to a NIP-54-style slug:
 * - Lowercase
 * - Replace any non-letter/digit with "-"
 * - Collapse multiple "-" into one
 * - Trim leading/trailing "-"
 */
export function normalizeSlug(input: string): string {
	const lower = input.toLowerCase();
	const replaced = lower.replace(/[^a-z0-9]+/g, '-');
	const collapsed = replaced.replace(/-+/g, '-');
	return collapsed.replace(/^-+|-+$/g, '');
}

/**
 * Generate a random, URL-safe identifier for "d" tag.
 * Uses crypto.getRandomValues if available; falls back to Math.random (weaker).
 */
export function randomId(bytes: number = 9): string {
	const len = Math.max(1, Math.min(bytes, 64));
	let arr: Uint8Array;
	if (typeof globalThis !== 'undefined' && globalThis.crypto?.getRandomValues) {
		arr = new Uint8Array(len);
		globalThis.crypto.getRandomValues(arr);
	} else {
		// Fallback: not cryptographically strong
		arr = new Uint8Array(len);
		for (let i = 0; i < len; i++) {
			arr[i] = Math.floor(Math.random() * 256);
		}
	}
	// Base64url encode without padding
	let b64 =
		typeof Buffer !== 'undefined'
			? Buffer.from(arr).toString('base64')
			: btoa(String.fromCharCode(...arr));
	return b64.replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/g, '');
}

/* ------------------------------- d tag helpers ------------------------------ */

/**
 * Extract "d" tag value from an event’s tags, if any.
 */
export function extractDFromTags(tags: NostrTag[]): string | undefined {
	for (const t of tags) {
		if (t[0] === 'd' && typeof t[1] === 'string' && t[1].length > 0) {
			return t[1];
		}
	}
	return undefined;
}

/**
 * Ensure a "d" identifier for a PRE event.
 * - If tags already contain a "d", return it.
 * - If a source string is given, return a normalized slug from it.
 * - Otherwise, return a random ID.
 */
export function ensureD(
	tags: NostrTag[],
	source?: string | null,
	options?: { randomBytes?: number }
): string {
	const existing = extractDFromTags(tags);
	if (existing) return existing;
	if (source && source.trim().length > 0) {
		const slug = normalizeSlug(source.trim());
		return slug.length > 0 ? slug : randomId(options?.randomBytes ?? 9);
	}
	return randomId(options?.randomBytes ?? 9);
}

/* ---------------------------- a-pointer (kind:d) ---------------------------- */

/**
 * Compute the "a" pointer string "kind:pubkey_hex:d".
 */
export function makeAPointer(kind: number, pubkeyHex: string, d: string): string {
	if (!Number.isInteger(kind) || kind < 0 || kind > 0xffff) {
		throw new Error(`Invalid kind: ${kind}`);
	}
	if (!isHex64(pubkeyHex)) {
		throw new Error(`Invalid pubkey hex (expected 64 lowercase hex chars): ${pubkeyHex}`);
	}
	if (!d || typeof d !== 'string') {
		throw new Error(`Invalid d tag: ${d}`);
	}
	return `${kind}:${pubkeyHex}:${d}`;
}

/**
 * Parse an "a" pointer "kind:pubkey_hex:d".
 */
export function parseAPointer(a: string): APointer {
	const parts = a.split(':');
	if (parts.length !== 3) {
		throw new Error(`Invalid a-pointer format: ${a}`);
	}
	const kind = Number(parts[0]);
	const pubkey = parts[1];
	const d = parts[2];
	if (!Number.isInteger(kind)) {
		throw new Error(`Invalid kind in a-pointer: ${parts[0]}`);
	}
	if (!isHex64(pubkey)) {
		throw new Error(`Invalid pubkey in a-pointer: ${pubkey}`);
	}
	if (!d) {
		throw new Error(`Invalid d in a-pointer: ${d}`);
	}
	return { kind, pubkey, d };
}

/**
 * Build an "a" pointer for an event. Requires that "d" is present (PRE).
 */
export function makeAPointerForEvent(evt: NostrEventMinimal): string {
	const d = extractDFromTags(evt.tags);
	if (!d) {
		throw new Error(`Missing "d" tag for PRE event of kind ${evt.kind}`);
	}
	return makeAPointer(evt.kind, evt.pubkey, d);
}

/* ------------------------------ naddr-like (UI) ----------------------------- */

/**
 * Compute a human-friendly "naddr-like" string that carries the tuple
 * (kind, pubkey, d) with optional relays, without doing bech32 encoding.
 *
 * Format:
 * naddr-like:kind:pubkey_hex:d[?relays=relay1,relay2,...]
 *
 * This is useful as a placeholder until a proper NIP-19 encoder is available.
 */
export function makeNaddrLike(
	kind: number,
	pubkeyHex: string,
	d: string,
	relays?: string[] | null
): string {
	const a = makeAPointer(kind, pubkeyHex, d);
	if (relays && relays.length > 0) {
		const encodedRelays = relays.map((r) => encodeURIComponent(r)).join(',');
		return `naddr-like:${a}?relays=${encodedRelays}`;
	}
	return `naddr-like:${a}`;
}

/**
 * Build naddr-like for an event. Requires "d".
 * You can pass relays if you have them (e.g., from list/set definitions).
 */
export function makeNaddrLikeForEvent(evt: NostrEventMinimal, relays?: string[] | null): string {
	const d = extractDFromTags(evt.tags);
	if (!d) {
		throw new Error(`Missing "d" tag for PRE event of kind ${evt.kind}`);
	}
	return makeNaddrLike(evt.kind, evt.pubkey, d, relays);
}

/**
 * Parse a naddr-like string back to a tuple and (optionally) relays.
 */
export function parseNaddrLike(naddrLike: string): { a: APointer; relays?: string[] } {
	if (!naddrLike.startsWith('naddr-like:')) {
		throw new Error(`Invalid naddr-like prefix: ${naddrLike}`);
	}
	const rest = naddrLike.slice('naddr-like:'.length);
	const [aPart, qs] = rest.split('?', 2);
	const a = parseAPointer(aPart);
	if (!qs) return { a };
	const params = new URLSearchParams(qs);
	const relaysParam = params.get('relays');
	const relays = relaysParam ? relaysParam.split(',').map(decodeURIComponent) : undefined;
	return { a, relays };
}

/* ------------------------------ PRE kind checks ----------------------------- */

export function isPreKind(kind: number): boolean {
	return Number.isInteger(kind) && kind >= 30000 && kind < 40000;
}

/* ---------------------------- Event tag utilities --------------------------- */

/**
 * Insert or replace a "d" tag into tags immutably, returning a new array.
 * If no "d" exists, it’s appended; if it exists, the first occurrence is replaced.
 */
export function upsertDTag(tags: NostrTag[], d: string): NostrTag[] {
	const out: NostrTag[] = [];
	let replaced = false;
	for (const t of tags) {
		if (!replaced && t[0] === 'd') {
			out.push(['d', d]);
			replaced = true;
		} else {
			out.push(t);
		}
	}
	if (!replaced) out.push(['d', d]);
	return out;
}

/**
 * Convenience: ensure a d-tag on a PRE template (immutably).
 * - If present, it is returned unchanged.
 * - If absent, a slug is generated from source or a random id.
 */
export function ensureDTagForPre(
	tags: NostrTag[],
	source?: string | null,
	options?: { randomBytes?: number }
): NostrTag[] {
	const existing = extractDFromTags(tags);
	if (existing) return tags.slice();
	const d = ensureD(tags, source, options);
	return upsertDTag(tags, d);
}
