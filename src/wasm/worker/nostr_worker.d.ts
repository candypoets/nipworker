/* tslint:disable */
/* eslint-disable */
export function init_nostr_client(): Promise<NostrClient>;
/**
 * EOSE (End of Stored Events) represents the completion of stored events delivery
 * This matches the Go type from types/eose.go
 */
export class EOSE {
  free(): void;
  constructor(total_connections: number, remaining_connections: number);
  /**
   * Check if all connections are complete (remaining connections is 0)
   */
  is_complete(): boolean;
  /**
   * Get the number of completed connections
   */
  completed_connections(): number;
  /**
   * Get the completion percentage (0.0 to 1.0)
   */
  completion_percentage(): number;
  /**
   * Convert to JSON string
   */
  to_json(): string;
  /**
   * Create from JSON string
   */
  static from_json(json: string): EOSE;
  total_connections: number;
  remaining_connections: number;
}
export class NostrClient {
  private constructor();
  free(): void;
  static new(): Promise<NostrClient>;
  open_subscription(subscription_id: string, requests_data: Uint8Array, shared_buffer: SharedArrayBuffer): Promise<void>;
  close_subscription(subscription_id: string): Promise<void>;
  set_signer(signer_type: string, private_key: string): void;
  get_public_key(): void;
  get_active_subscription_count(): Promise<number>;
  get_connection_count(): Promise<number>;
  handle_message(message_obj: any): Promise<void>;
}

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
  readonly memory: WebAssembly.Memory;
  readonly __wbg_eose_free: (a: number, b: number) => void;
  readonly __wbg_get_eose_total_connections: (a: number) => number;
  readonly __wbg_set_eose_total_connections: (a: number, b: number) => void;
  readonly __wbg_get_eose_remaining_connections: (a: number) => number;
  readonly __wbg_set_eose_remaining_connections: (a: number, b: number) => void;
  readonly eose_new: (a: number, b: number) => number;
  readonly eose_is_complete: (a: number) => number;
  readonly eose_completed_connections: (a: number) => number;
  readonly eose_completion_percentage: (a: number) => number;
  readonly eose_to_json: (a: number, b: number) => void;
  readonly eose_from_json: (a: number, b: number, c: number) => void;
  readonly __wbg_nostrclient_free: (a: number, b: number) => void;
  readonly nostrclient_new: () => number;
  readonly nostrclient_open_subscription: (a: number, b: number, c: number, d: number, e: number, f: number) => number;
  readonly nostrclient_close_subscription: (a: number, b: number, c: number) => number;
  readonly nostrclient_set_signer: (a: number, b: number, c: number, d: number, e: number, f: number) => void;
  readonly nostrclient_get_public_key: (a: number, b: number) => void;
  readonly nostrclient_get_active_subscription_count: (a: number) => number;
  readonly nostrclient_get_connection_count: (a: number) => number;
  readonly nostrclient_handle_message: (a: number, b: number) => number;
  readonly init_nostr_client: () => number;
  readonly rustsecp256k1_v0_8_1_context_create: (a: number) => number;
  readonly rustsecp256k1_v0_8_1_context_destroy: (a: number) => void;
  readonly rustsecp256k1_v0_8_1_default_illegal_callback_fn: (a: number, b: number) => void;
  readonly rustsecp256k1_v0_8_1_default_error_callback_fn: (a: number, b: number) => void;
  readonly __wbindgen_export_0: (a: number) => void;
  readonly __wbindgen_export_1: (a: number, b: number, c: number) => void;
  readonly __wbindgen_export_2: (a: number, b: number) => number;
  readonly __wbindgen_export_3: (a: number, b: number, c: number, d: number) => number;
  readonly __wbindgen_export_4: WebAssembly.Table;
  readonly __wbindgen_add_to_stack_pointer: (a: number) => number;
  readonly __wbindgen_export_5: (a: number, b: number, c: number) => void;
  readonly __wbindgen_export_6: (a: number, b: number) => void;
  readonly __wbindgen_export_7: (a: number, b: number, c: number) => void;
  readonly __wbindgen_export_8: (a: number, b: number, c: number) => void;
  readonly __wbindgen_export_9: (a: number, b: number, c: number) => void;
  readonly __wbindgen_export_10: (a: number, b: number) => void;
  readonly __wbindgen_export_11: (a: number, b: number, c: number, d: number) => void;
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
