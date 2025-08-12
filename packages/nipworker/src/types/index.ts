export {
  type MainToWorkerMessage,
  type PipeConfig,
  type PipelineConfig,
  type RelayStatusUpdate,
  type Request,
  type SubscriptionConfig,
  type WorkerToMainMessage,
} from "@candypoets/rust-main";

import type { NostrEvent } from "nostr-tools";

import { type Kind0Parsed } from "./kind0";
import { type Kind1Parsed } from "./kind1";
import { type Kind10002Parsed } from "./kind10002";
import { type Kind10019Parsed } from "./kind10019";
import { type Kind17Parsed } from "./kind17";
import { type Kind17375Parsed } from "./kind17375";
import { type Kind3Parsed } from "./kind3";
import { type Kind39089Parsed } from "./kind39089";
import { type Kind4Parsed } from "./kind4";
import { type Kind6Parsed } from "./kind6";
import { type Kind7Parsed } from "./kind7";
import { type Kind7374Parsed } from "./kind7374";
import { type Kind7375Parsed } from "./kind7375";
import type { Kind7376Parsed } from "./kind7376";
import { type Kind9321Parsed } from "./kind9321";
import { type Kind9735Parsed } from "./kind9735";

export * from "./proofs";

export * from "./kind0";
export * from "./kind1";
export * from "./kind10002";
export * from "./kind10019";
export * from "./kind17";
export * from "./kind17375";
export * from "./kind3";
export * from "./kind39089";
export * from "./kind4";
export * from "./kind6";
export * from "./kind7";
export * from "./kind7374";
export * from "./kind7375";
export * from "./kind7376";
export * from "./kind9321";
export * from "./kind9735";

export type ParsedEvent<T> = NostrEvent & {
  parsed?: T | null;
  requests?: Request[];
  relays?: string[];
};

export type ContentBlock = {
  type:
    | "text"
    | "image"
    | "video"
    | "mediaGrid"
    | "code"
    | "link"
    | "npub"
    | "nprofile"
    | "note"
    | "nevent"
    | "naddr"
    | "hashtag"
    | "cashu";
  text: string;
  data?: Record<string, any>;
};

export type AnyKind =
  | Kind0Parsed
  | Kind1Parsed
  | Kind3Parsed
  | Kind4Parsed
  | Kind6Parsed
  | Kind7Parsed
  | Kind17Parsed
  | Kind9735Parsed
  | Kind9321Parsed // For Kind9321 which seems to use Kind1Parsed
  | Kind10002Parsed
  | Kind10019Parsed
  | Kind17375Parsed
  | Kind7374Parsed
  | Kind7375Parsed
  | Kind7376Parsed
  | Kind39089Parsed;

export type SubscribeKind =
  | "CACHED_EVENT"
  | "FETCHED_EVENT"
  | "COUNT"
  | "CONNECTION_STATUS"
  | "EOCE"
  | "BUFFER_FULL";

export type PublishKind = "PUBLISH_STATUS";

export type SignerType = string;

// Enum-like object for SignerType
export const SignerTypes = {
  PK: "privkey" as SignerType,
  // SignerTypeNone: "none" as SignerType
} as const;
