import {
	WorkerMessage,
	MessageType,
	ConnectionStatus,
	Eoce,
	CountResponse,
	BufferFull,
	ValidProofs,
	ParsedEvent,
	Kind0Parsed,
	Kind1Parsed,
	Kind3Parsed,
	Kind4Parsed,
	Kind6Parsed,
	Kind7Parsed,
	Kind17Parsed,
	Kind10002Parsed,
	Kind10019Parsed,
	Kind17375Parsed,
	Kind7374Parsed,
	Kind7375Parsed,
	Kind7376Parsed,
	Kind9321Parsed,
	Kind9735Parsed,
	ContentBlock,
	LinkPreviewData,
	ContentData,
	CashuData,
	HashtagData,
	CodeData,
	MediaGroupData,
	NostrData,
	VideoData,
	NostrEvent,
	ListParsed
} from 'src/generated/nostr/fb';
import { unionToContentData } from 'src/generated/nostr/fb/content-data';
import { unionToMessage } from 'src/generated/nostr/fb/message';
import { unionToParsedData, ParsedData } from 'src/generated/nostr/fb/parsed-data';

// ---- Top-level Message helpers ----
export function isConnectionStatus(msg: WorkerMessage): ConnectionStatus | null {
	if (msg.type() !== MessageType.ConnectionStatus) return null;
	return unionToMessage(msg.contentType(), msg.content.bind(msg)) as ConnectionStatus;
}

export const asConnectionStatus = isConnectionStatus;

export function isEoce(msg: WorkerMessage): Eoce | null {
	if (msg.type() !== MessageType.Eoce) return null;
	return msg.content(new Eoce()) ?? null;
}

export const asEoce = isEoce;

export function isCountResponse(msg: WorkerMessage): CountResponse | null {
	if (msg.type() !== MessageType.CountResponse) return null;
	return msg.content(new CountResponse()) ?? null;
}

export const asCountResponse = isCountResponse;

export function isBufferFull(msg: WorkerMessage): BufferFull | null {
	if (msg.type() !== MessageType.BufferFull) return null;
	return msg.content(new BufferFull()) ?? null;
}

export const asBufferFull = isBufferFull;

export function isValidProofs(msg: WorkerMessage): ValidProofs | null {
	if (msg.type() !== MessageType.ValidProofs) return null;
	return msg.content(new ValidProofs()) ?? null;
}

export const asValidProofs = isValidProofs;

// ---- Generic ParsedEvent --------
export function isParsedEvent(msg: WorkerMessage): ParsedEvent | null {
	if (msg.type() !== MessageType.ParsedNostrEvent) return null;
	return unionToMessage(msg.contentType(), msg.content.bind(msg)) as ParsedEvent;
}

export const asParsedEvent = isParsedEvent;

export const isNostrEvent = (msg: WorkerMessage): NostrEvent | null => {
	if (msg.type() !== MessageType.NostrEvent) return null;
	return unionToMessage(msg.contentType(), msg.content.bind(msg)) as NostrEvent;
};

export const asNostrEvent = isNostrEvent;

// ---- ParsedData Kind helpers ----
function parsedKind<T>(msg: WorkerMessage, kind: ParsedData): T | null {
	if (!msg) return null;
	const ev = isParsedEvent(msg);
	if (!ev) return null;
	if (ev?.parsedType?.() !== kind) return null;
	return unionToParsedData(kind, ev.parsed.bind(ev)) as T;
}

export function isKind0(msg: WorkerMessage): Kind0Parsed | null {
	return parsedKind<Kind0Parsed>(msg, ParsedData.Kind0Parsed);
}
export function asKind0(ev: ParsedEvent): Kind0Parsed | null {
	if (!ev) return null;
	if (ev?.parsedType?.() !== ParsedData.Kind0Parsed) return null;
	return unionToParsedData(ParsedData.Kind0Parsed, ev.parsed.bind(ev)) as Kind0Parsed;
}
export function isKind1(msg: WorkerMessage): Kind1Parsed | null {
	return parsedKind<Kind1Parsed>(msg, ParsedData.Kind1Parsed);
}
export function asKind1(ev: ParsedEvent): Kind1Parsed | null {
	if (ev?.parsedType?.() !== ParsedData.Kind1Parsed) return null;
	return unionToParsedData(ParsedData.Kind1Parsed, ev.parsed.bind(ev)) as Kind1Parsed;
}
export function isKind3(msg: WorkerMessage): Kind3Parsed | null {
	return parsedKind<Kind3Parsed>(msg, ParsedData.Kind3Parsed);
}
export function asKind3(ev: ParsedEvent): Kind3Parsed | null {
	if (ev?.parsedType?.() !== ParsedData.Kind3Parsed) return null;
	return unionToParsedData(ParsedData.Kind3Parsed, ev.parsed.bind(ev)) as Kind3Parsed;
}
export function isKind4(msg: WorkerMessage): Kind4Parsed | null {
	return parsedKind<Kind4Parsed>(msg, ParsedData.Kind4Parsed);
}
export function asKind4(ev: ParsedEvent): Kind4Parsed | null {
	if (ev?.parsedType?.() !== ParsedData.Kind4Parsed) return null;
	return unionToParsedData(ParsedData.Kind4Parsed, ev.parsed.bind(ev)) as Kind4Parsed;
}
export function isKind6(msg: WorkerMessage): Kind6Parsed | null {
	return parsedKind<Kind6Parsed>(msg, ParsedData.Kind6Parsed);
}
export function asKind6(ev: ParsedEvent): Kind6Parsed | null {
	if (ev?.parsedType?.() !== ParsedData.Kind6Parsed) return null;
	return unionToParsedData(ParsedData.Kind6Parsed, ev.parsed.bind(ev)) as Kind6Parsed;
}
export function isKind7(msg: WorkerMessage): Kind7Parsed | null {
	return parsedKind<Kind7Parsed>(msg, ParsedData.Kind7Parsed);
}
export function asKind7(ev: ParsedEvent): Kind7Parsed | null {
	if (ev?.parsedType?.() !== ParsedData.Kind7Parsed) return null;
	return unionToParsedData(ParsedData.Kind7Parsed, ev.parsed.bind(ev)) as Kind7Parsed;
}
export function isKind17(msg: WorkerMessage): Kind17Parsed | null {
	return parsedKind<Kind17Parsed>(msg, ParsedData.Kind17Parsed);
}
export function asKind17(ev: ParsedEvent): Kind17Parsed | null {
	if (ev?.parsedType?.() !== ParsedData.Kind17Parsed) return null;
	return unionToParsedData(ParsedData.Kind17Parsed, ev.parsed.bind(ev)) as Kind17Parsed;
}
export function isKind10002(msg: WorkerMessage): Kind10002Parsed | null {
	return parsedKind<Kind10002Parsed>(msg, ParsedData.Kind10002Parsed);
}
export function asKind10002(ev: ParsedEvent): Kind10002Parsed | null {
	if (ev?.parsedType?.() !== ParsedData.Kind10002Parsed) return null;
	return unionToParsedData(ParsedData.Kind10002Parsed, ev.parsed.bind(ev)) as Kind10002Parsed;
}

export function isKind10019(msg: WorkerMessage): Kind10019Parsed | null {
	return parsedKind<Kind10019Parsed>(msg, ParsedData.Kind10019Parsed);
}
export function asKind10019(ev: ParsedEvent): Kind10019Parsed | null {
	if (ev?.parsedType?.() !== ParsedData.Kind10019Parsed) return null;
	return unionToParsedData(ParsedData.Kind10019Parsed, ev.parsed.bind(ev)) as Kind10019Parsed;
}

export function isKind17375(msg: WorkerMessage): Kind17375Parsed | null {
	return parsedKind<Kind17375Parsed>(msg, ParsedData.Kind17375Parsed);
}
export function asKind17375(ev: ParsedEvent): Kind17375Parsed | null {
	if (ev?.parsedType?.() !== ParsedData.Kind17375Parsed) return null;
	return unionToParsedData(ParsedData.Kind17375Parsed, ev.parsed.bind(ev)) as Kind17375Parsed;
}

export function isKind7374(msg: WorkerMessage): Kind7374Parsed | null {
	return parsedKind<Kind7374Parsed>(msg, ParsedData.Kind7374Parsed);
}
export function asKind7374(ev: ParsedEvent): Kind7374Parsed | null {
	if (ev?.parsedType?.() !== ParsedData.Kind7374Parsed) return null;
	return unionToParsedData(ParsedData.Kind7374Parsed, ev.parsed.bind(ev)) as Kind7374Parsed;
}

export function isKind7375(msg: WorkerMessage): Kind7375Parsed | null {
	return parsedKind<Kind7375Parsed>(msg, ParsedData.Kind7375Parsed);
}
export function asKind7375(ev: ParsedEvent): Kind7375Parsed | null {
	if (ev?.parsedType?.() !== ParsedData.Kind7375Parsed) return null;
	return unionToParsedData(ParsedData.Kind7375Parsed, ev.parsed.bind(ev)) as Kind7375Parsed;
}

export function isKind7376(msg: WorkerMessage): Kind7376Parsed | null {
	return parsedKind<Kind7376Parsed>(msg, ParsedData.Kind7376Parsed);
}
export function asKind7376(ev: ParsedEvent): Kind7376Parsed | null {
	if (ev?.parsedType?.() !== ParsedData.Kind7376Parsed) return null;
	return unionToParsedData(ParsedData.Kind7376Parsed, ev.parsed.bind(ev)) as Kind7376Parsed;
}

export function isKind9321(msg: WorkerMessage): Kind9321Parsed | null {
	return parsedKind<Kind9321Parsed>(msg, ParsedData.Kind9321Parsed);
}
export function asKind9321(ev: ParsedEvent): Kind9321Parsed | null {
	if (ev?.parsedType?.() !== ParsedData.Kind9321Parsed) return null;
	return unionToParsedData(ParsedData.Kind9321Parsed, ev.parsed.bind(ev)) as Kind9321Parsed;
}

export function isKind9735(msg: WorkerMessage): Kind9735Parsed | null {
	return parsedKind<Kind9735Parsed>(msg, ParsedData.Kind9735Parsed);
}
export function asKind9735(ev: ParsedEvent): Kind9735Parsed | null {
	if (ev?.parsedType?.() !== ParsedData.Kind9735Parsed) return null;
	return unionToParsedData(ParsedData.Kind9735Parsed, ev.parsed.bind(ev)) as Kind9735Parsed;
}

export function isNip51(msg: WorkerMessage): ListParsed | null {
	return parsedKind<ListParsed>(msg, ParsedData.ListParsed);
}

export function asNip51(ev: ParsedEvent): ListParsed | null {
	if (ev?.parsedType?.() !== ParsedData.ListParsed) return null;
	return unionToParsedData(ParsedData.ListParsed, ev.parsed.bind(ev)) as ListParsed;
}

export function asCodeData(block: ContentBlock): CodeData | null {
	if (block.dataType() !== ContentData.CodeData) return null;
	return unionToContentData(ContentData.CodeData, block.data.bind(block)) as CodeData;
}

export function asHashtagData(block: ContentBlock): HashtagData | null {
	if (block.dataType() !== ContentData.HashtagData) return null;
	return unionToContentData(ContentData.HashtagData, block.data.bind(block)) as HashtagData;
}

export function asCashuData(block: ContentBlock): CashuData | null {
	if (block.dataType() !== ContentData.CashuData) return null;
	return unionToContentData(ContentData.CashuData, block.data.bind(block)) as CashuData;
}

export function asImageData(block: ContentBlock): ImageData | null {
	if (block.dataType() !== ContentData.ImageData) return null;
	return unionToContentData(ContentData.ImageData, block.data.bind(block)) as unknown as ImageData;
}

export function asVideoData(block: ContentBlock): VideoData | null {
	if (block.dataType() !== ContentData.VideoData) return null;
	return unionToContentData(ContentData.VideoData, block.data.bind(block)) as VideoData;
}

export function asMediaGroupData(block: ContentBlock): MediaGroupData | null {
	if (block.dataType() !== ContentData.MediaGroupData) return null;
	return unionToContentData(ContentData.MediaGroupData, block.data.bind(block)) as MediaGroupData;
}

export function asNostrData(block: ContentBlock): NostrData | null {
	if (block.dataType() !== ContentData.NostrData) return null;
	return unionToContentData(ContentData.NostrData, block.data.bind(block)) as NostrData;
}

export function asLinkPreview(block: ContentBlock): LinkPreviewData | null {
	if (block.dataType() !== ContentData.LinkPreviewData) return null;
	return unionToContentData(ContentData.LinkPreviewData, block.data.bind(block)) as LinkPreviewData;
}
