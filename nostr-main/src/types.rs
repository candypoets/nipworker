use std::collections::HashMap;

use crate::proof::ProofUnion;
use nostr::Event;

use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

/// Publish status types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PublishStatus {
    Pending,
    Sent,
    Success,
    Failed,
    Rejected,
    ConnectionError,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayStatusUpdate {
    pub relay: String,
    pub status: PublishStatus,
    pub message: String,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventTemplate {
    pub kind: u64,
    pub content: String,
    pub tags: Vec<Vec<String>>,
}

/// EOSE (End of Stored Events) represents the completion of stored events delivery
/// This matches the Go type from types/eose.go
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[wasm_bindgen]
pub struct EOSE {
    #[serde(rename = "totalConnections")]
    #[wasm_bindgen(getter_with_clone)]
    pub total_connections: i32,

    #[serde(rename = "remainingConnections")]
    #[wasm_bindgen(getter_with_clone)]
    pub remaining_connections: i32,
}

/// Subscription event types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SubscribeKind {
    #[serde(rename = "CACHED_EVENT")]
    CachedEvent,
    #[serde(rename = "FETCHED_EVENT")]
    FetchedEvent,
    #[serde(rename = "COUNT")]
    Count,
    #[serde(rename = "EOSE")]
    Eose,
    #[serde(rename = "EOCE")]
    Eoce,
}

/// Publish event types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PublishKind {
    #[serde(rename = "PUBLISH_STATUS")]
    PublishStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Request {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub ids: Vec<String>,

    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub authors: Vec<String>,

    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub kinds: Vec<i32>,

    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub tags: HashMap<String, Vec<String>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<i64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<i64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i32>,

    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub search: String,

    pub relays: Vec<String>,

    #[serde(rename = "closeOnEOSE", default)]
    pub close_on_eose: bool,

    #[serde(rename = "cacheFirst", default)]
    pub cache_first: bool,

    #[serde(rename = "noOptimize", default)]
    pub no_optimize: bool,

    #[serde(default)]
    pub count: bool,

    #[serde(rename = "noContext", default)]
    pub no_context: bool,
}

/// ParsedEvent represents a Nostr event with additional parsed data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedEvent {
    #[serde(flatten)]
    pub event: Event,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub parsed: Option<serde_json::Value>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub requests: Option<Vec<Request>>,

    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub relays: Vec<String>,
}

/// Messages sent from worker to main thread
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkerToMainMessage {
    SubscriptionEvent {
        subscription_id: String,
        event_type: SubscribeKind,
        event_data: Vec<Vec<ParsedEvent>>,
    },
    PublishStatus {
        publish_id: String,
        status: Vec<RelayStatusUpdate>,
    },
    SignedEvent {
        content: String,
        signed_event: serde_json::Value,
    },
    Count {
        subscription_id: String,
        count: u32,
    },
    Proofs {
        mint: String,
        proofs: Vec<ProofUnion>,
    },
    Eose {
        subscription_id: String,
        data: EOSE,
    },
    Eoce {
        subscription_id: String,
    },
    PublicKey {
        public_key: String,
    },
}

/// Messages sent from main thread to worker
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MainToWorkerMessage {
    Subscribe {
        subscription_id: String,
        requests: Vec<Request>,
    },
    Unsubscribe {
        subscription_id: String,
    },
    Publish {
        publish_id: String,
        template: EventTemplate,
    },
    SignEvent {
        template: EventTemplate,
    },
    SetSigner {
        signer_type: String,
        private_key: String,
    },
    GetPublicKey,
}
