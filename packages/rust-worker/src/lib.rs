#![allow(async_fn_in_trait)]

use wasm_bindgen::prelude::*;

// Use `wee_alloc` as the global allocator for smaller WASM size
#[cfg(feature = "wee_alloc")]
#[global_allocator]
static ALLOC: wee_alloc::WeeAlloc = wee_alloc::WeeAlloc::INIT;

// Re-export common types and modules
pub mod config;
pub mod db;
pub mod network;
pub mod parser;
pub mod pipeline;
pub mod relays;
pub mod signer;
pub mod types;
pub mod utils;

// Re-export key types for external use
pub use db::NostrDB;
pub use network::NetworkManager;
pub use parser::Parser;
pub use signer::{PrivateKeySigner, SharedSignerManager, Signer, SignerManager, SignerManagerImpl};
use types::EventTemplate;
pub use types::*;

// Re-export communication types for external use
pub use types::thread::{MainToWorkerMessage, WorkerToMainMessage};

// Type aliases to match Go implementation
pub type NostrEvent = Event;

// Common error types
#[derive(Debug, thiserror::Error)]
pub enum NostrError {
    #[error("Database error: {0}")]
    Database(#[from] anyhow::Error),
    #[error("Network error: {0}")]
    Network(String),
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("Signer error: {0}")]
    Signer(String),
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("MessagePack error: {0}")]
    MessagePack(#[from] rmp_serde::encode::Error),
    #[error("MessagePack decode error: {0}")]
    MessagePackDecode(#[from] rmp_serde::decode::Error),
}

impl Into<JsValue> for NostrError {
    fn into(self) -> JsValue {
        JsValue::from_str(&self.to_string())
    }
}

// Common result type
pub type NostrResult<T> = Result<T, NostrError>;

// Common macros
#[macro_export]
macro_rules! console_log {
    ($($t:tt)*) => {
        web_sys::console::log_1(&format_args!($($t)*).to_string().into());
    }
}

#[macro_export]
macro_rules! console_error {
    ($($t:tt)*) => {
        web_sys::console::error_1(&format_args!($($t)*).to_string().into());
    }
}

// Worker implementation
use js_sys::{SharedArrayBuffer, Uint8Array};
use std::sync::{Arc, Once};
use tracing::info;

use crate::types::network::Request;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["self"])]
    fn postMessage(data: &JsValue);
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

static TRACING_INIT: Once = Once::new();

fn setup_tracing() {
    TRACING_INIT.call_once(|| {
        // Simple console writer for Web Workers
        struct ConsoleWriter;

        impl std::io::Write for ConsoleWriter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                let message = String::from_utf8_lossy(buf);
                web_sys::console::log_1(&JsValue::from_str(&message));
                Ok(buf.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        // Try to set up a simple subscriber - if it fails, just continue
        let _ = tracing_subscriber::fmt()
            .with_writer(|| ConsoleWriter)
            .without_time()
            .with_target(false)
            .with_max_level(tracing::Level::INFO)
            .try_init();

        console_log!("Tracing subscriber initialized for Web Worker");
    });
}

// Default relay configurations to match Go implementation
const DEFAULT_RELAYS: &[&str] = &[
    "wss://relay.snort.social",
    "wss://relay.damus.io",
    "wss://relay.primal.net",
];

const INDEXER_RELAYS: &[&str] = &[
    "wss://user.kindpag.es",
    "wss://relay.nos.social",
    "wss://purplepag.es",
    "wss://relay.nostr.band",
];

#[wasm_bindgen]
pub struct NostrClient {
    connection_registry: Arc<relays::ConnectionRegistry>,
    network_manager: Arc<NetworkManager>,
    signer_manager: SharedSignerManager,
}

#[wasm_bindgen]
impl NostrClient {
    pub async fn new() -> Self {
        // Set up enhanced panic handling
        setup_panic_hook();
        setup_tracing();

        info!("Initializing NostrClient...");
        let database = Arc::new(NostrDB::with_relays(
            DEFAULT_RELAYS.iter().map(|s| s.to_string()).collect(),
            INDEXER_RELAYS.iter().map(|s| s.to_string()).collect(),
        ));
        database
            .initialize()
            .await
            .map_err(|e| {
                console_error!("Failed to initialize database: {}", e);
                e
            })
            .expect("Database initialization failed");
        let shared_signer_manager: SharedSignerManager = Arc::new(SignerManagerImpl::new());
        let connection_registry = Arc::new(relays::ConnectionRegistry::new());
        let parser = Arc::new(Parser::new_with_signer(
            shared_signer_manager.clone(),
            database.clone(),
        ));
        let network_manager = Arc::new(NetworkManager::new(
            database.clone(),
            connection_registry.clone() as Arc<relays::ConnectionRegistry>,
            parser.clone(),
        ));

        info!("NostrClient components initialized");

        Self {
            connection_registry,
            network_manager,
            signer_manager: shared_signer_manager,
        }
    }

    pub async fn open_subscription(
        &self,
        subscription_id: String,
        requests_data: &[u8],
        shared_buffer: SharedArrayBuffer,
    ) -> Result<(), JsValue> {
        let requests: Vec<Request> = rmp_serde::from_slice(requests_data)
            .map_err(|e| JsValue::from_str(&format!("Failed to parse requests: {}", e)))?;

        self.network_manager
            .open_subscription(subscription_id, requests, shared_buffer, None)
            .await
            .map_err(|e| JsValue::from_str(&format!("Failed to open subscription: {}", e)))
    }

    pub async fn close_subscription(&self, subscription_id: String) -> Result<(), JsValue> {
        self.network_manager
            .close_subscription(subscription_id)
            .await
            .map_err(|e| JsValue::from_str(&format!("Failed to close subscription: {}", e)))
    }

    async fn publish_event(
        &self,
        publish_id: String,
        template: EventTemplate,
        shared_buffer: SharedArrayBuffer,
    ) -> Result<String, JsValue> {
        let mut event = self
            .signer_manager
            .unsign_event(template)
            .map_err(|e| JsValue::from_str(&format!("Failed to create unsigned event: {}", e)))?;

        let summary = self
            .network_manager
            .publish_event(publish_id, &mut event, shared_buffer)
            .await
            .map_err(|e| JsValue::from_str(&format!("Failed to publish event: {}", e)))?;

        serde_json::to_string(&summary)
            .map_err(|e| JsValue::from_str(&format!("Failed to serialize summary: {}", e)))
    }

    fn sign_event(&self, template: EventTemplate) -> Result<(), JsValue> {
        let mut event = self
            .signer_manager
            .unsign_event(template)
            .map_err(|e| JsValue::from_str(&format!("Failed to create unsigned event: {}", e)))?;

        // Sign the event using the signer manager
        let signed_event = self
            .signer_manager
            .sign_event(&mut event)
            .map_err(|e| JsValue::from_str(&format!("Failed to sign event: {}", e)))?;

        // Convert signed event back to JSON
        let signed_event_json = serde_json::to_value(&signed_event)
            .map_err(|e| JsValue::from_str(&format!("Failed to serialize signed event: {}", e)))?;

        // Send signed event back to main thread
        let signed_event_message = WorkerToMainMessage::SignedEvent {
            content: signed_event.content.clone(),
            signed_event: signed_event_json,
        };

        let data = match rmp_serde::to_vec_named(&signed_event_message) {
            Ok(msgpack) => msgpack,
            Err(e) => {
                console_error!("Failed to serialize signed event message: {}", e);
                return Err(JsValue::from_str(
                    "Failed to serialize signed event message",
                ));
            }
        };

        let uint8_array = Uint8Array::new_with_length(data.len() as u32);
        uint8_array.copy_from(&data);
        postMessage(&uint8_array.into());

        Ok(())
    }

    pub fn set_signer(&self, signer_type: String, private_key: String) -> Result<(), JsValue> {
        let signer_type_enum: SignerType = signer_type
            .parse()
            .map_err(|e| JsValue::from_str(&format!("Invalid signer type: {}", e)))?;

        self.signer_manager
            .set_signer(signer_type_enum, &private_key)
            .map_err(|e| JsValue::from_str(&format!("Failed to set signer: {}", e)))
    }

    pub fn get_public_key(&self) -> Result<(), JsValue> {
        // Get the public key from signer manager
        let pubkey = self
            .signer_manager
            .get_public_key()
            .map_err(|e| JsValue::from_str(&format!("Failed to get public key: {}", e)))?;

        let pubkey_message = WorkerToMainMessage::PublicKey { public_key: pubkey };

        let data = match rmp_serde::to_vec_named(&pubkey_message) {
            Ok(msgpack) => msgpack,
            Err(e) => {
                console_error!("Failed to serialize public key message: {}", e);
                return Err(JsValue::from_str("Failed to serialize public key message"));
            }
        };

        let uint8_array = Uint8Array::new_with_length(data.len() as u32);
        uint8_array.copy_from(&data);
        postMessage(&uint8_array.into());

        Ok(())
    }

    pub async fn get_active_subscription_count(&self) -> u32 {
        self.network_manager.get_active_subscription_count().await
    }

    pub async fn get_connection_count(&self) -> u32 {
        self.connection_registry
            .active_subscription_ids()
            .await
            .len() as u32
    }

    pub async fn handle_message(&self, message_obj: &JsValue) -> Result<(), JsValue> {
        // Check if this is the new format with serializedMessage and sharedBuffer
        if let Some(obj) = message_obj.dyn_ref::<js_sys::Object>() {
            if js_sys::Reflect::has(obj, &JsValue::from_str("serializedMessage")).unwrap_or(false)
                && js_sys::Reflect::has(obj, &JsValue::from_str("sharedBuffer")).unwrap_or(false)
            {
                // Extract serialized message
                let serialized_msg =
                    js_sys::Reflect::get(obj, &JsValue::from_str("serializedMessage"))?;
                let message_uint8 = js_sys::Uint8Array::from(serialized_msg);
                let mut message_bytes = vec![0u8; message_uint8.length() as usize];
                message_uint8.copy_to(&mut message_bytes);

                // Extract SharedArrayBuffer
                let shared_buffer = js_sys::Reflect::get(obj, &JsValue::from_str("sharedBuffer"))?;
                let shared_buffer = shared_buffer
                    .dyn_into::<js_sys::SharedArrayBuffer>()
                    .map_err(|_| JsValue::from_str("Invalid SharedArrayBuffer"))?;

                let main_message: MainToWorkerMessage = rmp_serde::from_slice(&message_bytes)
                    .map_err(|e| JsValue::from_str(&format!("Failed to decode message: {}", e)))?;

                match main_message {
                    MainToWorkerMessage::Subscribe {
                        subscription_id,
                        requests,
                        config,
                    } => {
                        let requests_data = rmp_serde::to_vec_named(&requests).map_err(|e| {
                            JsValue::from_str(&format!("Failed to serialize requests: {}", e))
                        })?;
                        self.open_subscription_with_config(
                            subscription_id,
                            &requests_data,
                            shared_buffer,
                            config,
                        )
                        .await?;
                    }
                    MainToWorkerMessage::Publish {
                        publish_id,
                        template,
                    } => {
                        self.publish_event(publish_id, template, shared_buffer)
                            .await?;
                    }
                    _ => {
                        return Err(JsValue::from_str(
                            "Only Subscribe adn Publish messages support new format",
                        ));
                    }
                }
                return Ok(());
            }
        }

        // Fall back to old format for other message types
        let message_bytes = if message_obj.is_instance_of::<Uint8Array>() {
            let uint8_array: Uint8Array = message_obj.clone().dyn_into()?;
            uint8_array.to_vec()
        } else {
            return Err(JsValue::from_str("Expected Uint8Array message"));
        };

        info!("Received message in worker: {} bytes", message_bytes.len());

        let main_message: MainToWorkerMessage = rmp_serde::from_slice(&message_bytes)
            .map_err(|e| JsValue::from_str(&format!("Failed to decode message: {}", e)))?;

        match main_message {
            MainToWorkerMessage::Subscribe {
                subscription_id: _,
                requests: _,
                config: _,
            } => {
                return Err(JsValue::from_str(
                    "Subscribe requires SharedArrayBuffer in new format",
                ));
            }
            MainToWorkerMessage::Unsubscribe { subscription_id } => {
                self.close_subscription(subscription_id).await?;
            }
            MainToWorkerMessage::Publish {
                publish_id: _,
                template: _,
            } => {
                return Err(JsValue::from_str(
                    "Publish requires SharedArrayBuffer in new format",
                ));
            }
            MainToWorkerMessage::SignEvent { template } => {
                self.sign_event(template)?;
            }
            MainToWorkerMessage::SetSigner {
                signer_type,
                private_key,
            } => {
                self.set_signer(signer_type, private_key)?;
            }
            MainToWorkerMessage::GetPublicKey {} => {
                self.get_public_key()?;
            }
        }

        Ok(())
    }

    async fn open_subscription_with_config(
        &self,
        subscription_id: String,
        requests_data: &[u8],
        shared_buffer: SharedArrayBuffer,
        config: Option<crate::types::thread::SubscriptionConfig>,
    ) -> Result<(), JsValue> {
        let requests: Vec<Request> = rmp_serde::from_slice(requests_data)
            .map_err(|e| JsValue::from_str(&format!("Failed to deserialize requests: {}", e)))?;

        self.network_manager
            .open_subscription(subscription_id, requests, shared_buffer, config)
            .await
            .map_err(|e| JsValue::from_str(&format!("Failed to open subscription: {}", e)))?;

        Ok(())
    }
}

// Expose a function to initialize NostrClient
#[wasm_bindgen]
pub async fn init_nostr_client() -> Result<NostrClient, JsValue> {
    match NostrClient::new().await {
        client => Ok(client),
        #[allow(unreachable_patterns)]
        _ => Err(JsValue::from_str("Failed to initialize NostrClient")),
    }
}
