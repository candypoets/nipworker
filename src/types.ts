export {
  type MainToWorkerMessage,
  type RelayStatusUpdate,
  type Request,
  type WorkerToMainMessage,
} from "nostr-main/pkg/nostr_main.js";

export type SubscribeKind =
  | "CACHED_EVENT"
  | "FETCHED_EVENT"
  | "COUNT"
  | "EOSE"
  | "EOCE";

export type PublishKind = "PUBLISH_STATUS";

export type AnyKind = number;

export interface ParsedEvent<T extends AnyKind> {
  id: string;
  pubkey: string;
  created_at: number;
  kind: T;
  tags: string[][];
  content: string;
  sig: string;
}

export type SignerType = string;

// Enum-like object for SignerType
export const SignerTypes = {
  PK: "privkey" as SignerType,
  // SignerTypeNone: "none" as SignerType
} as const;
