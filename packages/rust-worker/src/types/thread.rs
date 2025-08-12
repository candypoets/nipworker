//! Communication types for worker-main thread communication
//!
//! This module contains all the message types and related structures used for
//! communication between the web worker and main thread.

use serde::{Deserialize, Serialize};

use crate::{
    types::{network::SubscribeKind, Request, SerializableParsedEvent, EOSE},
    EventTemplate, ProofUnion, RelayStatusUpdate,
};

/// Configuration for individual pipes in the pipeline
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipeConfig {
    pub name: String,
    #[serde(default)]
    pub params: Option<serde_json::Value>,
}

/// Configuration for the entire pipeline
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineConfig {
    pub pipes: Vec<PipeConfig>,
}

/// Configuration for subscription behavior
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscriptionConfig {
    /// Pipeline configuration for event processing
    #[serde(default)]
    pub pipeline: Option<PipelineConfig>,

    /// Whether to close subscription when EOSE is received from all relays
    #[serde(rename = "closeOnEose", default)]
    pub close_on_eose: bool,

    /// Whether to process cached events first before network requests
    #[serde(rename = "cacheFirst", default = "default_cache_first")]
    pub cache_first: bool,

    /// Maximum time to wait for network responses (in milliseconds)
    #[serde(rename = "timeoutMs", default)]
    pub timeout_ms: Option<u64>,

    /// Maximum number of events to return
    #[serde(rename = "maxEvents", default)]
    pub max_events: Option<usize>,

    /// Whether to enable subscription optimization (merging similar requests)
    #[serde(rename = "enableOptimization", default = "default_true")]
    pub enable_optimization: bool,

    /// Whether to skip cache and go directly to network
    #[serde(rename = "skipCache", default)]
    pub skip_cache: bool,

    /// Force subscription even if similar one exists
    #[serde(default)]
    pub force: bool,

    /// Estimated bytes per event for buffer sizing
    #[serde(rename = "bytesPerEvent", default)]
    pub bytes_per_event: Option<usize>,
}

fn default_cache_first() -> bool {
    true
}
fn default_true() -> bool {
    true
}

impl Default for SubscriptionConfig {
    fn default() -> Self {
        Self {
            pipeline: None,
            close_on_eose: false,
            cache_first: true,
            timeout_ms: None,
            max_events: None,
            enable_optimization: true,
            skip_cache: false,
            force: false,
            bytes_per_event: None,
        }
    }
}

/// Messages sent from worker to main thread
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkerToMainMessage {
    SubscriptionEvent {
        event_type: SubscribeKind,
        event_data: Vec<Vec<SerializableParsedEvent>>,
    },
    SignedEvent {
        content: String,
        signed_event: serde_json::Value,
    },
    ConnectionStatus {
        status: String,
        message: String,
        relay_url: String,
    },
    Count {
        kind: u32,
        count: u32,
        you: bool,
        metadata: String,
    },
    Proofs {
        mint: String,
        proofs: Vec<ProofUnion>,
    },
    Eoce {},
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
        #[serde(default)]
        config: Option<SubscriptionConfig>,
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
