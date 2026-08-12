import { nip19 } from 'nostr-tools';

import { ContentBlockT } from 'src/generated/nostr/fb/content-block';
import { ContentData } from 'src/generated/nostr/fb/content-data';
import { CodeDataT } from 'src/generated/nostr/fb/code-data';
import { CashuDataT } from 'src/generated/nostr/fb/cashu-data';
import { HashtagDataT } from 'src/generated/nostr/fb/hashtag-data';
import { ImageDataT } from 'src/generated/nostr/fb/image-data';
import { VideoDataT } from 'src/generated/nostr/fb/video-data';
import { MediaGroupDataT } from 'src/generated/nostr/fb/media-group-data';
import { MediaItemT } from 'src/generated/nostr/fb/media-item';
import { LinkPreviewDataT } from 'src/generated/nostr/fb/link-preview-data';
import { NostrDataT } from 'src/generated/nostr/fb/nostr-data';
import { LightningDataT } from 'src/generated/nostr/fb/lightning-data';

type MatchProcessor = (
	match: RegExpExecArray
) => ContentBlockT | null | Promise<ContentBlockT | null>;

const textEncoder = new TextEncoder();
const bech32Charset = 'qpzry9x8gf2tvdw0s3jn54khce6mua7l';
const secp256k1Order = BigInt('0xfffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141');

function bech32PolymodStep(checksum: number, value: number): number {
	const generators = [0x3b6a57b2, 0x26508e6d, 0x1ea119fa, 0x3d4233dd, 0x2a1462b3];
	const top = checksum >>> 25;
	let next = (((checksum & 0x01ffffff) << 5) ^ value) >>> 0;

	for (let bit = 0; bit < generators.length; bit++) {
		if (((top >>> bit) & 1) !== 0) next = (next ^ generators[bit]!) >>> 0;
	}
	return next;
}

function isValidBolt11Hrp(hrp: string): boolean {
	const networkAndAmount = hrp.slice(2);
	if (!hrp.startsWith('ln')) return false;

	const network = ['bcrt', 'tbs', 'tb', 'bc'].find((candidate) =>
		networkAndAmount.startsWith(candidate)
	);
	if (!network) return false;

	const amount = networkAndAmount.slice(network.length);
	if (amount.length === 0) return true;

	const suffix = amount[amount.length - 1];
	const multipliers: Record<string, bigint> = {
		m: 1_000_000_000n,
		u: 1_000_000n,
		n: 1_000n,
		p: 1n
	};
	const multiplier = suffix && multipliers[suffix] ? multipliers[suffix] : 1_000_000_000_000n;
	const digits = suffix && multipliers[suffix] ? amount.slice(0, -1) : amount;
	if (!/^[1-9][0-9]*$/.test(digits)) return false;

	let rawAmount = 0n;
	const maxRawAmount = ((1n << 64n) - 1n) / multiplier;
	for (const digit of digits) {
		rawAmount = rawAmount * 10n + BigInt(digit);
		if (rawAmount > maxRawAmount) return false;
	}

	return (rawAmount * multiplier) % 10n === 0n;
}

function isValidSecp256k1Scalar(bytes: number[]): boolean {
	let scalar = 0n;
	for (const byte of bytes) scalar = (scalar << 8n) | BigInt(byte);
	return scalar > 0n && scalar < secp256k1Order;
}

// Recognize a checksum-valid, structurally well-formed BOLT11 envelope. Payment
// code must still verify its signature, features, expiry, and payment policy.
function isWellFormedBolt11(invoice: string): boolean {
	if (!/^[\x00-\x7f]+$/.test(invoice)) return false;
	if (invoice !== invoice.toLowerCase() && invoice !== invoice.toUpperCase()) return false;

	const normalized = invoice.toLowerCase();
	const separator = normalized.lastIndexOf('1');
	if (separator <= 0 || separator === normalized.length - 1) return false;

	const hrp = normalized.slice(0, separator);
	const data = normalized.slice(separator + 1);
	if (!isValidBolt11Hrp(hrp)) return false;
	if (data.length < 117) return false;

	let checksum = 1;
	for (const char of hrp) checksum = bech32PolymodStep(checksum, char.charCodeAt(0) >>> 5);
	checksum = bech32PolymodStep(checksum, 0);
	for (const char of hrp) checksum = bech32PolymodStep(checksum, char.charCodeAt(0) & 31);
	for (const char of data) {
		const value = bech32Charset.indexOf(char);
		if (value === -1) return false;
		checksum = bech32PolymodStep(checksum, value);
	}
	if (checksum !== 1) return false;

	const taggedEnd = data.length - 6 - 104;
	if (taggedEnd < 7) return false;

	let index = 7;
	let paymentHashes = 0;
	let descriptions = 0;
	while (index < taggedEnd) {
		if (index + 3 > taggedEnd) return false;
		const tag = bech32Charset.indexOf(data[index]!);
		const lengthHigh = bech32Charset.indexOf(data[index + 1]!);
		const lengthLow = bech32Charset.indexOf(data[index + 2]!);
		if (tag === -1 || lengthHigh === -1 || lengthLow === -1) return false;

		const fieldLength = (lengthHigh << 5) | lengthLow;
		index += 3;
		const fieldEnd = index + fieldLength;
		if (fieldEnd > taggedEnd) return false;

		if (tag === 1) {
			if (fieldLength !== 52 || (bech32Charset.indexOf(data[fieldEnd - 1]!) & 15) !== 0) {
				return false;
			}
			paymentHashes++;
		} else if (tag === 13) {
			descriptions++;
		} else if (tag === 23) {
			if (fieldLength !== 52 || (bech32Charset.indexOf(data[fieldEnd - 1]!) & 15) !== 0) {
				return false;
			}
			descriptions++;
		}
		index = fieldEnd;
	}
	if (paymentHashes !== 1 || descriptions !== 1) return false;

	let accumulator = 0;
	let bits = 0;
	const signatureBytes: number[] = [];
	for (const char of data.slice(taggedEnd, -6)) {
		const value = bech32Charset.indexOf(char);
		if (value === -1) return false;
		accumulator = (accumulator << 5) | value;
		bits += 5;
		if (bits >= 8) {
			bits -= 8;
			const decoded = (accumulator >>> bits) & 0xff;
			signatureBytes.push(decoded);
			accumulator &= (1 << bits) - 1;
		}
	}

	return (
		signatureBytes.length === 65 &&
		bits === 0 &&
		signatureBytes[64]! <= 3 &&
		isValidSecp256k1Scalar(signatureBytes.slice(0, 32)) &&
		isValidSecp256k1Scalar(signatureBytes.slice(32, 64))
	);
}

export async function parseContent(content: string): Promise<ContentBlockT[]> {
	const blocks: ContentBlockT[] = [];

	// Helpers
	const textBlock = (text: string): ContentBlockT =>
		new ContentBlockT(textEncoder.encode('text'), textEncoder.encode(text), ContentData.NONE, null);

	const imageBlock = (url: string): ContentBlockT =>
		new ContentBlockT(
			textEncoder.encode('image'),
			textEncoder.encode(url),
			ContentData.ImageData,
			new ImageDataT(textEncoder.encode(url), null)
		);

	const videoBlock = (url: string): ContentBlockT =>
		new ContentBlockT(
			textEncoder.encode('video'),
			textEncoder.encode(url),
			ContentData.VideoData,
			new VideoDataT(textEncoder.encode(url), null)
		);

	const codeBlock = (raw: string, full: string): ContentBlockT => {
		// Try to extract optional language from first line of fenced code
		// Supports patterns like ```lang\ncode\n```
		const nl = raw.indexOf('\n');
		let language: string | null = null;
		let code = raw;
		if (nl !== -1) {
			const firstLine = raw.slice(0, nl).trim();
			const rest = raw.slice(nl + 1);
			if (firstLine && /^[a-zA-Z0-9+#\.\-_]+$/.test(firstLine)) {
				language = firstLine;
				code = rest;
			}
		}

		return new ContentBlockT(
			textEncoder.encode('code'),
			textEncoder.encode(full),
			ContentData.CodeData,
			new CodeDataT(textEncoder.encode(language || ''), textEncoder.encode(code))
		);
	};

	const cashuBlock = (token: string): ContentBlockT =>
		new ContentBlockT(
			textEncoder.encode('cashu'),
			textEncoder.encode(token),
			ContentData.CashuData,
			new CashuDataT(textEncoder.encode(token))
		);

	const lightningBlock = (text: string): ContentBlockT | null => {
		const invoice = text.slice(0, 10).toLowerCase() === 'lightning:' ? text.slice(10) : text;

		if (!isWellFormedBolt11(invoice)) return null;

		return new ContentBlockT(
			textEncoder.encode('lightning'),
			textEncoder.encode(text),
			ContentData.LightningData,
			new LightningDataT(textEncoder.encode(invoice))
		);
	};

	const hashtagBlock = (tag: string): ContentBlockT =>
		new ContentBlockT(
			textEncoder.encode('hashtag'),
			textEncoder.encode(`#${tag}`),
			ContentData.HashtagData,
			new HashtagDataT(textEncoder.encode(tag))
		);

	const linkBlock = (url: string): ContentBlockT =>
		new ContentBlockT(
			textEncoder.encode('link'),
			textEncoder.encode(url),
			ContentData.LinkPreviewData,
			new LinkPreviewDataT(textEncoder.encode(url), null, null, null)
		);

	const nostrBlock = (bech32: string, fullText: string): ContentBlockT | null => {
		try {
			const decoded = nip19.decode(bech32);
			const type = decoded.type as 'npub' | 'nprofile' | 'note' | 'nevent' | 'naddr';

			let id: string | null = null;
			let relays: string[] = [];
			let author: string | null = null;
			let kind: bigint = BigInt(0);

			const d = decoded.data as any;

			switch (type) {
				case 'npub':
					// data: hex pubkey
					id = d as string;
					break;
				case 'nprofile':
					// data: { pubkey, relays? }
					id = d.pubkey;
					relays = Array.isArray(d.relays) ? d.relays : [];
					break;
				case 'note':
					// data: hex event id
					id = d as string;
					break;
				case 'nevent':
					// data: { id, relays?, author?, kind? }
					id = d.id;
					relays = Array.isArray(d.relays) ? d.relays : [];
					author = typeof d.author === 'string' ? d.author : null;
					if (typeof d.kind === 'number') kind = BigInt(d.kind);
					break;
				case 'naddr':
					// data: { identifier, pubkey, kind, relays? }
					// Build a stable id
					id = `${d.kind}:${d.pubkey}:${d.identifier}`;
					relays = Array.isArray(d.relays) ? d.relays : [];
					if (typeof d.kind === 'number') kind = BigInt(d.kind);
					author = typeof d.pubkey === 'string' ? d.pubkey : null;
					break;
			}

			// Ensure required fields for NostrDataT: id and entity are required
			if (!id) {
				id = bech32;
			}

			return new ContentBlockT(
				textEncoder.encode(type),
				textEncoder.encode(fullText),
				ContentData.NostrData,
				new NostrDataT(
					textEncoder.encode(id),
					textEncoder.encode(bech32),
					relays,
					textEncoder.encode(author || ''),
					kind
				)
			);
		} catch {
			// Skip invalid NIP-19 candidates so another recognized token inside
			// the same span can still be classified.
			return null;
		}
	};

	// Define all the patterns we want to match
	const patterns: Array<{
		type: string;
		regex: RegExp;
		processMatch: MatchProcessor;
	}> = [
		{
			type: 'code',
			regex: /```([\s\S]*?)```/g,
			processMatch: (match) => codeBlock(match[1] || '', match[0])
		},
		{
			type: 'cashu',
			regex: /(cashuA[A-Za-z0-9_-]+)/g,
			processMatch: (match) => cashuBlock(match[0])
		},
		{
			type: 'hashtag',
			// Match hashtags that are not part of a URL
			regex: /(?<![^\s"'(])(#[a-zA-Z0-9_]+)(?![a-zA-Z0-9_])/g,
			processMatch: (match) => hashtagBlock(match[0].substring(1))
		},
		{
			type: 'image',
			regex: /(https?:\/\/\S+\.(?:jpg|jpeg|png|gif|webp|svg|ico)(?:\?\S*)?)/gi,
			processMatch: (match) => imageBlock(match[0])
		},
		{
			type: 'video',
			regex: /(https?:\/\/\S+\.(?:mp4|mov|avi|mkv|webm|m4v)(?:\?\S*)?)/gi,
			processMatch: (match) => videoBlock(match[0])
		},
		{
			type: 'nostr',
			regex: /nostr:([a-z0-9]+)/gi,
			processMatch: (match) => {
				const bech32 = match[1];
				return nostrBlock(bech32 || '', match[0]);
			}
		},
		{
			type: 'link',
			regex: /(https?:\/\/\S+)(?![\)])/gi,
			processMatch: async (match) => linkBlock(match[0])
		},
		{
			type: 'lightning',
			// BOLT11: optional lightning: URI scheme, Bitcoin network HRP, and Bech32 data.
			// ASCII word boundaries avoid extracting an invoice-shaped substring from a larger token.
			regex:
				/\b(?:lightning:)?ln(?:bcrt|tbs|tb|bc)(?:[1-9][0-9]*[munp]?)?1[023456789ac-hj-np-z]{117,}\b/gi,
			processMatch: (match) => lightningBlock(match[0])
		}
	];

	// Find all matches with their positions
	const allMatches: Array<{
		start: number;
		end: number;
		block: ContentBlockT;
	}> = [];

	// First, find all matches for all patterns
	for (const pattern of patterns) {
		let match: RegExpExecArray | null;
		pattern.regex.lastIndex = 0;

		while ((match = pattern.regex.exec(content)) !== null) {
			const start = match.index;
			const end = start + match[0].length;
			const block = await pattern.processMatch(match);
			if (!block) continue;

			allMatches.push({ start, end, block });
		}
	}

	// Sort matches by start position
	allMatches.sort((a, b) => a.start - b.start);

	// Remove overlapping matches (prioritize earlier patterns in the array)
	const filteredMatches: typeof allMatches = [];

	for (const match of allMatches) {
		const overlaps = filteredMatches.some(
			(existing) =>
				(match.start >= existing.start && match.start < existing.end) ||
				(match.end > existing.start && match.end <= existing.end) ||
				(match.start <= existing.start && match.end >= existing.end)
		);

		if (!overlaps) {
			filteredMatches.push(match);
		}
	}

	// Re-sort filtered matches
	filteredMatches.sort((a, b) => a.start - b.start);

	// Build the final result, including text between matches
	let lastIndex = 0;

	for (const { start, end, block } of filteredMatches) {
		// Add text before this match
		if (start > lastIndex) {
			blocks.push(textBlock(content.substring(lastIndex, start)));
		}

		// Add the match
		blocks.push(block);

		lastIndex = end;
	}

	// Add any remaining text after the last match
	if (lastIndex < content.length) {
		blocks.push(textBlock(content.substring(lastIndex)));
	}

	// Post-processing: group consecutive media into grids
	const processedBlocks: ContentBlockT[] = [];
	let mediaGroup: ContentBlockT[] = [];

	const isWhitespace = (s: string) => /^\s+$/.test(s);

	for (let i = 0; i < blocks.length; i++) {
		const block = blocks[i];

		// If this is an image or video
		if (block?.type?.toString() === 'image' || block?.type?.toString() === 'video') {
			mediaGroup.push(block);
			continue;
		}

		// If this is whitespace or newlines between media, check what follows
		if (
			block?.type?.toString() === 'text' &&
			typeof block?.text?.toString() === 'string' &&
			isWhitespace(block.text?.toString())
		) {
			if (
				mediaGroup.length > 0 &&
				i + 1 < blocks.length &&
				(blocks[i + 1]?.type?.toString() === 'image' || blocks[i + 1]?.type?.toString() === 'video')
			) {
				continue;
			}
		}

		// If we have collected media and the current block breaks the sequence
		if (mediaGroup.length > 0) {
			// Add media group if it contains more than one item
			if (mediaGroup.length > 1) {
				const items: MediaItemT[] = mediaGroup.map((m) => {
					if (m.dataType === ContentData.ImageData) {
						const d = m.data as ImageDataT;
						return new MediaItemT(
							new ImageDataT(textEncoder.encode(d.url ?? (m.text as string)), null),
							null
						);
					} else if (m.dataType === ContentData.VideoData) {
						const d = m.data as VideoDataT;
						return new MediaItemT(
							null,
							new VideoDataT(textEncoder.encode(d.url ?? (m.text as string)), null)
						);
					}
					// Fallback — shouldn't occur because we only collect image/video
					return new MediaItemT(null, null);
				});

				const text = mediaGroup.map((m) => String(m.text ?? '')).join('\n');
				processedBlocks.push(
					new ContentBlockT(
						textEncoder.encode('mediaGrid'),
						textEncoder.encode(text),
						ContentData.MediaGroupData,
						new MediaGroupDataT(items)
					)
				);
			} else {
				// Just add the single media item
				processedBlocks.push(mediaGroup[0]);
			}
			mediaGroup = [];
		}

		// Add the current non-media block
		processedBlocks.push(block);
	}

	// Don't forget any remaining media
	if (mediaGroup.length > 0) {
		if (mediaGroup.length > 1) {
			const items: MediaItemT[] = mediaGroup.map((m) => {
				if (m.dataType === ContentData.ImageData) {
					const d = m.data as ImageDataT;
					return new MediaItemT(
						new ImageDataT(textEncoder.encode(d.url ?? (m.text as string)), null),
						null
					);
				} else if (m.dataType === ContentData.VideoData) {
					const d = m.data as VideoDataT;
					return new MediaItemT(
						null,
						new VideoDataT(textEncoder.encode(d.url ?? (m.text as string)), null)
					);
				}
				return new MediaItemT(null, null);
			});

			const text = mediaGroup.map((m) => String(m.text ?? '')).join('\n');
			processedBlocks.push(
				new ContentBlockT(
					textEncoder.encode('mediaGrid'),
					textEncoder.encode(text),
					ContentData.MediaGroupData,
					new MediaGroupDataT(items)
				)
			);
		} else {
			processedBlocks.push(mediaGroup[0]);
		}
	}

	return processedBlocks;
}
