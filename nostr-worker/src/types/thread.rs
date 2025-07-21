//! Communication types for worker-main thread communication
//!
//! This module contains all the message types and related structures used for
//! communication between the web worker and main thread.

use serde::{Deserialize, Serialize};

use crate::{
    types::{network::SubscribeKind, Request, SerializableParsedEvent, EOSE},
    EventTemplate, RelayStatusUpdate,
};

/// Messages sent from worker to main thread
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkerToMainMessage {
    SubscriptionEvent {
        subscription_id: String,
        event_type: SubscribeKind,
        event_data: Vec<Vec<SerializableParsedEvent>>,
    },
    PublishStatus {
        publish_id: String,
        status: Vec<RelayStatusUpdate>,
    },
    SignedEvent {
        content: String,
        signed_event: serde_json::Value,
    },
    Debug {
        message: String,
        data: serde_json::Value,
    },
    Count {
        subscription_id: String,
        count: u32,
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
