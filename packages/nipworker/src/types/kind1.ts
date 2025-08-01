import { parseReferences } from "nostr-tools";
import type { EventPointer, ProfilePointer } from "nostr-tools/nip19";
import { ContentBlock } from ".";

type Reference = ReturnType<typeof parseReferences>[0];

export type Kind1Parsed = {
  content: string;
  parsedContent: ContentBlock[];
  shortenedContent: ContentBlock[];
  references: Reference[];
  quotes: ProfilePointer[];
  mentions: EventPointer[];
  reply?: EventPointer | undefined; // direct reply
  root?: EventPointer | undefined; // thread root
};
