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
	Kind20Parsed,
	Kind22Parsed,
	Kind1111Parsed,
	Kind10002Parsed,
	Kind10019Parsed,
	Kind17375Parsed,
	Kind7374Parsed,
	Kind7375Parsed,
	Kind7376Parsed,
	Kind9321Parsed,
	Kind9735Parsed,
	Kind30023Parsed,
	Kind1311Parsed,
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
	ListParsed,
	PreGenericParsed
} from 'src/generated/nostr/fb';
import { ImageData as FbImageData } from 'src/generated/nostr/fb';
import { ParsedData } from 'src/generated/nostr/fb/parsed-data';

// ---- Top-level Message helpers ----
export function isConnectionStatus(msg: WorkerMessage): ConnectionStatus | null {
	if (msg.type() !== MessageType.ConnectionStatus) return null;
	return msg.content(new ConnectionStatus()) ?? null;
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
	return msg.content(new ParsedEvent()) ?? null;
}

export const asParsedEvent = isParsedEvent;

export const isNostrEvent = (msg: WorkerMessage): NostrEvent | null => {
	if (msg.type() !== MessageType.NostrEvent) return null;
	return msg.content(new NostrEvent()) ?? null;
};

export const asNostrEvent = isNostrEvent;

// ---- ParsedData Kind helpers ----
function parsedKind<T>(msg: WorkerMessage, kind: ParsedData): T | null {
	if (!msg) return null;
	const ev = isParsedEvent(msg);
	if (!ev) return null;
	const parsedType = ev.parsedType();
	if (parsedType !== kind) return null;

	switch (kind) {
		case ParsedData.Kind0Parsed:
			return ev.parsed(new Kind0Parsed()) as T | null;
		case ParsedData.Kind1Parsed:
			return ev.parsed(new Kind1Parsed()) as T | null;
		case ParsedData.Kind3Parsed:
			return ev.parsed(new Kind3Parsed()) as T | null;
		case ParsedData.Kind4Parsed:
			return ev.parsed(new Kind4Parsed()) as T | null;
		case ParsedData.Kind6Parsed:
			return ev.parsed(new Kind6Parsed()) as T | null;
		case ParsedData.Kind7Parsed:
			return ev.parsed(new Kind7Parsed()) as T | null;
		case ParsedData.Kind17Parsed:
			return ev.parsed(new Kind17Parsed()) as T | null;
		case ParsedData.Kind20Parsed:
			return ev.parsed(new Kind20Parsed()) as T | null;
		case ParsedData.Kind22Parsed:
			return ev.parsed(new Kind22Parsed()) as T | null;
		case ParsedData.Kind1111Parsed:
			return ev.parsed(new Kind1111Parsed()) as T | null;
		case ParsedData.Kind1311Parsed:
			return ev.parsed(new Kind1311Parsed()) as T | null;
		case ParsedData.Kind10002Parsed:
			return ev.parsed(new Kind10002Parsed()) as T | null;
		case ParsedData.Kind10019Parsed:
			return ev.parsed(new Kind10019Parsed()) as T | null;
		case ParsedData.Kind17375Parsed:
			return ev.parsed(new Kind17375Parsed()) as T | null;
		case ParsedData.Kind7374Parsed:
			return ev.parsed(new Kind7374Parsed()) as T | null;
		case ParsedData.Kind7375Parsed:
			return ev.parsed(new Kind7375Parsed()) as T | null;
		case ParsedData.Kind7376Parsed:
			return ev.parsed(new Kind7376Parsed()) as T | null;
		case ParsedData.Kind9321Parsed:
			return ev.parsed(new Kind9321Parsed()) as T | null;
		case ParsedData.Kind9735Parsed:
			return ev.parsed(new Kind9735Parsed()) as T | null;
		case ParsedData.Kind30023Parsed:
			return ev.parsed(new Kind30023Parsed()) as T | null;
		case ParsedData.ListParsed:
			return ev.parsed(new ListParsed()) as T | null;
		case ParsedData.PreGenericParsed:
			return ev.parsed(new PreGenericParsed()) as T | null;
		default:
			return null;
	}
}

export function isKind0(msg: WorkerMessage): Kind0Parsed | null {
	return parsedKind<Kind0Parsed>(msg, ParsedData.Kind0Parsed);
}
export function asKind0(ev: ParsedEvent): Kind0Parsed | null {
	if (!ev) return null;
	if (ev.parsedType() !== ParsedData.Kind0Parsed) return null;
	return ev.parsed(new Kind0Parsed()) ?? null;
}
export function isKind1(msg: WorkerMessage): Kind1Parsed | null {
	return parsedKind<Kind1Parsed>(msg, ParsedData.Kind1Parsed);
}
export function asKind1(ev: ParsedEvent): Kind1Parsed | null {
	if (ev.parsedType() !== ParsedData.Kind1Parsed) return null;
	return ev.parsed(new Kind1Parsed()) ?? null;
}
export function isKind3(msg: WorkerMessage): Kind3Parsed | null {
	return parsedKind<Kind3Parsed>(msg, ParsedData.Kind3Parsed);
}
export function asKind3(ev: ParsedEvent): Kind3Parsed | null {
	if (ev.parsedType() !== ParsedData.Kind3Parsed) return null;
	return ev.parsed(new Kind3Parsed()) ?? null;
}
export function isKind4(msg: WorkerMessage): Kind4Parsed | null {
	return parsedKind<Kind4Parsed>(msg, ParsedData.Kind4Parsed);
}
export function asKind4(ev: ParsedEvent): Kind4Parsed | null {
	if (ev.parsedType() !== ParsedData.Kind4Parsed) return null;
	return ev.parsed(new Kind4Parsed()) ?? null;
}
export function isKind6(msg: WorkerMessage): Kind6Parsed | null {
	return parsedKind<Kind6Parsed>(msg, ParsedData.Kind6Parsed);
}
export function asKind6(ev: ParsedEvent): Kind6Parsed | null {
	if (ev.parsedType() !== ParsedData.Kind6Parsed) return null;
	return ev.parsed(new Kind6Parsed()) ?? null;
}
export function isKind7(msg: WorkerMessage): Kind7Parsed | null {
	return parsedKind<Kind7Parsed>(msg, ParsedData.Kind7Parsed);
}
export function asKind7(ev: ParsedEvent): Kind7Parsed | null {
	if (ev.parsedType() !== ParsedData.Kind7Parsed) return null;
	return ev.parsed(new Kind7Parsed()) ?? null;
}
export function isKind17(msg: WorkerMessage): Kind17Parsed | null {
	return parsedKind<Kind17Parsed>(msg, ParsedData.Kind17Parsed);
}
export function asKind17(ev: ParsedEvent): Kind17Parsed | null {
	if (ev.parsedType() !== ParsedData.Kind17Parsed) return null;
	return ev.parsed(new Kind17Parsed()) ?? null;
}

export function isKind20(msg: WorkerMessage): Kind20Parsed | null {
	return parsedKind<Kind20Parsed>(msg, ParsedData.Kind20Parsed);
}
export function asKind20(ev: ParsedEvent): Kind20Parsed | null {
	if (ev.parsedType() !== ParsedData.Kind20Parsed) return null;
	return ev.parsed(new Kind20Parsed()) ?? null;
}

export function isKind22(msg: WorkerMessage): Kind22Parsed | null {
	return parsedKind<Kind22Parsed>(msg, ParsedData.Kind22Parsed);
}
export function asKind22(ev: ParsedEvent): Kind22Parsed | null {
	if (ev.parsedType() !== ParsedData.Kind22Parsed) return null;
	return ev.parsed(new Kind22Parsed()) ?? null;
}

export function isKind1111(msg: WorkerMessage): Kind1111Parsed | null {
	return parsedKind<Kind1111Parsed>(msg, ParsedData.Kind1111Parsed);
}
export function asKind1111(ev: ParsedEvent): Kind1111Parsed | null {
	if (ev.parsedType() !== ParsedData.Kind1111Parsed) return null;
	return ev.parsed(new Kind1111Parsed()) ?? null;
}
export function isKind1311(msg: WorkerMessage): Kind1311Parsed | null {
	return parsedKind<Kind1311Parsed>(msg, ParsedData.Kind1311Parsed);
}
export function asKind1311(ev: ParsedEvent): Kind1311Parsed | null {
	if (ev.parsedType() !== ParsedData.Kind1311Parsed) return null;
	return ev.parsed(new Kind1311Parsed()) ?? null;
}
export function isKind10002(msg: WorkerMessage): Kind10002Parsed | null {
	return parsedKind<Kind10002Parsed>(msg, ParsedData.Kind10002Parsed);
}
export function asKind10002(ev: ParsedEvent): Kind10002Parsed | null {
	if (ev.parsedType() !== ParsedData.Kind10002Parsed) return null;
	return ev.parsed(new Kind10002Parsed()) ?? null;
}

export function isKind10019(msg: WorkerMessage): Kind10019Parsed | null {
	return parsedKind<Kind10019Parsed>(msg, ParsedData.Kind10019Parsed);
}
export function asKind10019(ev: ParsedEvent): Kind10019Parsed | null {
	if (ev.parsedType() !== ParsedData.Kind10019Parsed) return null;
	return ev.parsed(new Kind10019Parsed()) ?? null;
}

export function isKind17375(msg: WorkerMessage): Kind17375Parsed | null {
	return parsedKind<Kind17375Parsed>(msg, ParsedData.Kind17375Parsed);
}
export function asKind17375(ev: ParsedEvent): Kind17375Parsed | null {
	if (ev.parsedType() !== ParsedData.Kind17375Parsed) return null;
	return ev.parsed(new Kind17375Parsed()) ?? null;
}

export function isKind7374(msg: WorkerMessage): Kind7374Parsed | null {
	return parsedKind<Kind7374Parsed>(msg, ParsedData.Kind7374Parsed);
}
export function asKind7374(ev: ParsedEvent): Kind7374Parsed | null {
	if (ev.parsedType() !== ParsedData.Kind7374Parsed) return null;
	return ev.parsed(new Kind7374Parsed()) ?? null;
}

export function isKind7375(msg: WorkerMessage): Kind7375Parsed | null {
	return parsedKind<Kind7375Parsed>(msg, ParsedData.Kind7375Parsed);
}
export function asKind7375(ev: ParsedEvent): Kind7375Parsed | null {
	if (ev.parsedType() !== ParsedData.Kind7375Parsed) return null;
	return ev.parsed(new Kind7375Parsed()) ?? null;
}

export function isKind7376(msg: WorkerMessage): Kind7376Parsed | null {
	return parsedKind<Kind7376Parsed>(msg, ParsedData.Kind7376Parsed);
}
export function asKind7376(ev: ParsedEvent): Kind7376Parsed | null {
	if (ev.parsedType() !== ParsedData.Kind7376Parsed) return null;
	return ev.parsed(new Kind7376Parsed()) ?? null;
}

export function isKind9321(msg: WorkerMessage): Kind9321Parsed | null {
	return parsedKind<Kind9321Parsed>(msg, ParsedData.Kind9321Parsed);
}
export function asKind9321(ev: ParsedEvent): Kind9321Parsed | null {
	if (ev.parsedType() !== ParsedData.Kind9321Parsed) return null;
	return ev.parsed(new Kind9321Parsed()) ?? null;
}

export function isKind9735(msg: WorkerMessage): Kind9735Parsed | null {
	return parsedKind<Kind9735Parsed>(msg, ParsedData.Kind9735Parsed);
}
export function asKind9735(ev: ParsedEvent): Kind9735Parsed | null {
	if (ev.parsedType() !== ParsedData.Kind9735Parsed) return null;
	return ev.parsed(new Kind9735Parsed()) ?? null;
}

export function isNip51(msg: WorkerMessage): ListParsed | null {
	return parsedKind<ListParsed>(msg, ParsedData.ListParsed);
}

export function asNip51(ev: ParsedEvent): ListParsed | null {
	if (ev.parsedType() !== ParsedData.ListParsed) return null;
	return ev.parsed(new ListParsed()) ?? null;
}

export function isPreGeneric(msg: WorkerMessage): PreGenericParsed | null {
	return parsedKind<PreGenericParsed>(msg, ParsedData.PreGenericParsed);
}

export function asPreGeneric(ev: ParsedEvent): PreGenericParsed | null {
	if (ev.parsedType() !== ParsedData.PreGenericParsed) return null;
	return ev.parsed(new PreGenericParsed()) ?? null;
}

export function asCodeData(block: ContentBlock): CodeData | null {
	if (block.dataType() !== ContentData.CodeData) return null;
	return block.data(new CodeData()) ?? null;
}

export function asHashtagData(block: ContentBlock): HashtagData | null {
	if (block.dataType() !== ContentData.HashtagData) return null;
	return block.data(new HashtagData()) ?? null;
}

export function asCashuData(block: ContentBlock): CashuData | null {
	if (block.dataType() !== ContentData.CashuData) return null;
	return block.data(new CashuData()) ?? null;
}

export function asImageData(block: ContentBlock): FbImageData | null {
	if (block.dataType() !== ContentData.ImageData) return null;
	return block.data(new FbImageData()) ?? null;
}

export function asVideoData(block: ContentBlock): VideoData | null {
	if (block.dataType() !== ContentData.VideoData) return null;
	return block.data(new VideoData()) ?? null;
}

export function asMediaGroupData(block: ContentBlock): MediaGroupData | null {
	if (block.dataType() !== ContentData.MediaGroupData) return null;
	return block.data(new MediaGroupData()) ?? null;
}

export function asNostrData(block: ContentBlock): NostrData | null {
	if (block.dataType() !== ContentData.NostrData) return null;
	return block.data(new NostrData()) ?? null;
}

export function asLinkPreview(block: ContentBlock): LinkPreviewData | null {
	if (block.dataType() !== ContentData.LinkPreviewData) return null;
	return block.data(new LinkPreviewData()) ?? null;
}
