/* tslint:disable */
/* eslint-disable */
export function encodeAndPostMessage(worker: Worker, message_js: any): void;
export function decodeWorkerToMainMessage(buffer: Uint8Array): any;
export function init(): void;

export type Request = {
  ids?: string[];
  authors?: string[];
  kinds?: number[];
  tags?: Record<string, string[]>;
  since?: number;
  until?: number;
  limit?: number;
  search?: string;
  relays: string[];
  closeOnEOSE?: boolean;
  cacheFirst?: boolean;
  noOptimize?: boolean;
  count?: boolean;
  noContext?: boolean;
};

export type SubscribeKind = "CACHED_EVENT" | "FETCHED_EVENT" | "COUNT" | "EOSE" | "EOCE";

export type PublishStatus = "Pending" | "Sent" | "Success" | "Failed" | "Rejected" | "ConnectionError";

export type RelayStatusUpdate = {
  relay: string;
  status: PublishStatus;
  message: string;
  timestamp: number;
};

export type EOSE = {
  totalConnections: number;
  remainingConnections: number;
};

export type EventTemplate = {
  kind: number;
  content: string;
  tags: string[][];
};

export type MainToWorkerMessage =
  | { Subscribe: { subscription_id: string; requests: Request[] } }
  | { Unsubscribe: { subscription_id: string } }
  | { Publish: { publish_id: string; template: EventTemplate } }
  | { SignEvent: { template: EventTemplate } }
  | { GetPublicKey: {} }
  | { SetSigner: { signer_type: string; private_key: string } };

export type WorkerToMainMessage =
  | { SubscriptionEvent: { subscription_id: string; event_type: SubscribeKind; event_data: any[] } }
  | { PublishStatus: { publish_id: string; status: RelayStatusUpdate[] } }
  | { SignedEvent: { content: string; signed_event: any } }
  | { Debug: { message: string; data: any } }
  | { Count: { subscription_id: string; count: number } }
  | { Eose: { subscription_id: string; data: EOSE } }
  | { Eoce: { subscription_id: string } }
  | { PublicKey: { public_key: string } };


/**
 * EOSE (End of Stored Events) represents the completion of stored events delivery
 * This matches the Go type from types/eose.go
 */
export class EOSE {
  private constructor();
  free(): void;
  total_connections: number;
  remaining_connections: number;
}

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
  readonly memory: WebAssembly.Memory;
  readonly __wbg_eose_free: (a: number, b: number) => void;
  readonly __wbg_get_eose_total_connections: (a: number) => number;
  readonly __wbg_set_eose_total_connections: (a: number, b: number) => void;
  readonly __wbg_get_eose_remaining_connections: (a: number) => number;
  readonly __wbg_set_eose_remaining_connections: (a: number, b: number) => void;
  readonly encodeAndPostMessage: (a: number, b: number, c: number) => void;
  readonly decodeWorkerToMainMessage: (a: number, b: number, c: number) => void;
  readonly init: () => void;
  readonly rustsecp256k1_v0_8_1_context_create: (a: number) => number;
  readonly rustsecp256k1_v0_8_1_context_destroy: (a: number) => void;
  readonly rustsecp256k1_v0_8_1_default_illegal_callback_fn: (a: number, b: number) => void;
  readonly rustsecp256k1_v0_8_1_default_error_callback_fn: (a: number, b: number) => void;
  readonly __wbindgen_export_0: (a: number, b: number) => number;
  readonly __wbindgen_export_1: (a: number, b: number, c: number, d: number) => number;
  readonly __wbindgen_export_2: (a: number) => void;
  readonly __wbindgen_export_3: (a: number, b: number, c: number) => void;
  readonly __wbindgen_add_to_stack_pointer: (a: number) => number;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;
/**
* Instantiates the given `module`, which can either be bytes or
* a precompiled `WebAssembly.Module`.
*
* @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
*
* @returns {InitOutput}
*/
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
* If `module_or_path` is {RequestInfo} or {URL}, makes a request and
* for everything else, calls `WebAssembly.instantiate` directly.
*
* @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
*
* @returns {Promise<InitOutput>}
*/
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
