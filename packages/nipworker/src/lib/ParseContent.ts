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

type MatchProcessor = (match: RegExpExecArray) => ContentBlockT | Promise<ContentBlockT>;

const textEncoder = new TextEncoder();

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

	const nostrBlock = (bech32: string, fullText: string): ContentBlockT => {
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
			// Fallback to plain text block when decode fails
			return textBlock(fullText);
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
