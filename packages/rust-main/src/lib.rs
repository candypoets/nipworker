mod proof;
mod types;
mod utils;

use js_sys::Uint8Array;
use rmp_serde::{from_slice, to_vec_named};
use wasm_bindgen::prelude::*;
use web_sys::Worker;

pub use crate::proof::ProofUnion;
pub use crate::types::{MainToWorkerMessage, WorkerToMainMessage};

#[cfg(feature = "wee_alloc")]
#[global_allocator]
static ALLOC: wee_alloc::WeeAlloc = wee_alloc::WeeAlloc::INIT;

#[macro_export]
macro_rules! console_error {
    ($($t:tt)*) => (web_sys::console::error_1(&format_args!($($t)*).to_string().into()))
}

fn setup_panic_hook() {
    std::panic::set_hook(Box::new(|panic_info| {
        let mut message = String::new();

        // Get location information
        if let Some(location) = panic_info.location() {
            message.push_str(&format!(
                "RUST PANIC in '{}' at line {}, column {}: ",
                location.file(),
                location.line(),
                location.column()
            ));
        } else {
            message.push_str("RUST PANIC at unknown location: ");
        }

        // Get panic message
        if let Some(payload) = panic_info.payload().downcast_ref::<&str>() {
            message.push_str(payload);
        } else if let Some(payload) = panic_info.payload().downcast_ref::<String>() {
            message.push_str(payload);
        } else {
            message.push_str("Unknown panic payload");
        }

        console_error!("{}", message);

        // Also use the console_error_panic_hook for browser integration
        console_error_panic_hook::hook(panic_info);
    }));
}

fn post_message_to_worker(worker: &Worker, message: MainToWorkerMessage) -> Result<(), JsValue> {
    let encoded_message = to_vec_named(&message).map_err(|e| {
        let error_message = format!("Failed to serialize message with msgpack: {}", e);
        console_error!("{}", error_message);
        JsValue::from_str(&error_message)
    })?;

    let uint8_array = Uint8Array::from(&encoded_message[..]);

    worker.post_message(&uint8_array).map_err(|e| {
        console_error!("Failed to post message to worker: {:?}", e);
        e
    })
}

#[wasm_bindgen(js_name = encodeAndPostMessage)]
pub fn encode_and_post_message(worker: &Worker, message_js: JsValue) -> Result<(), JsValue> {
    let message: MainToWorkerMessage =
        serde_wasm_bindgen::from_value(message_js.clone()).map_err(|e| {
            let error_message = format!("Failed to deserialize message from JsValue: {}", e);
            console_error!("{}, {:?}", error_message, message_js);
            JsValue::from_str(&error_message)
        })?;
    post_message_to_worker(worker, message)
}

#[wasm_bindgen(js_name = decodeWorkerToMainMessage)]
pub fn decode_worker_to_main_message(buffer: &[u8]) -> Result<JsValue, JsValue> {
    let message: WorkerToMainMessage = from_slice(buffer).map_err(|e| {
        let error_message = format!("Failed to deserialize message with msgpack: {}", e);
        console_error!("{}", error_message);
        JsValue::from_str(&error_message)
    })?;
    serde_wasm_bindgen::to_value(&message).map_err(|e| {
        let error_message = format!("Failed to serialize message to JsValue: {}", e);
        console_error!("{}", error_message);
        JsValue::from_str(&error_message)
    })
}

#[wasm_bindgen(js_name = init)]
pub fn init() {
    setup_panic_hook();
}

// TypeScript type generation helpers
#[wasm_bindgen(typescript_custom_section)]
const TS_APPEND_CONTENT: &'static str = r#"
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

export type SubscribeKind = "CACHED_EVENT" | "FETCHED_EVENT" | "COUNT" | "EOSE" | "EOCE" | "BUFFER_FULL";

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

export type PipeConfig = {
  name: string;
  params?: Record<string, any>;
};

export type PipelineConfig = {
  pipes: PipeConfig[];
};

export type SubscriptionConfig = {
  pipeline?: PipelineConfig;
  closeOnEose?: boolean;
  cacheFirst?: boolean;
  timeoutMs?: number;
  maxEvents?: number;
  enableOptimization?: boolean;
  skipCache?: boolean;
  force?: boolean;
  bytesPerEvent?: number;
};

export type DleqProof = {
  e: string;
  s: string;
  r?: string;
};

export type ProofUnion = {
  version?: number;
  amount?: number;
  secret?: string;
  C?: string;
  id?: string;
  dleq?: DleqProof;
  p2pksigs?: string[];
  htlcpreimage?: string;
};

export type MainToWorkerMessage =
  | { Subscribe: { subscription_id: string; requests: Request[]; config?: SubscriptionConfig } }
  | { Unsubscribe: { subscription_id: string } }
  | { Publish: { publish_id: string; template: EventTemplate } }
  | { SignEvent: { template: EventTemplate } }
  | { GetPublicKey: {} }
  | { SetSigner: { signer_type: string; private_key: string } };

export type WorkerToMainMessage =
  | { SubscriptionEvent: { event_type: SubscribeKind; event_data: any[] } }
  | { PublishStatus: { publish_id: string; status: RelayStatusUpdate[] } }
  | { SignedEvent: { content: string; signed_event: any } }
  | { Debug: { message: string; data: any } }
  | { Count: { kind: number; count: number; you: boolean; metadata: string } }
  | { Eose: { data: EOSE } }
  | { Eoce: {} }
  | { PublicKey: { public_key: string } }
  | { Proofs: { mint: string; proofs: ProofUnion[] } };
"#;
