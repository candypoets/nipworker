#![allow(async_fn_in_trait)]

use flatbuffers::FlatBufferBuilder;
use js_sys::SharedArrayBuffer;
use shared::{telemetry, SabRing};
use wasm_bindgen::prelude::*;

pub mod generated;
pub mod network;
pub mod parser;
pub mod pipeline;
pub mod relays;
pub mod signer;
pub mod types;
pub mod utils;

// Re-export key types for external use
pub use network::NetworkManager;
pub use parser::Parser;
pub use signer::{PrivateKeySigner, SignerInterface, SignerManager, SignerManagerInterface};

pub use types::*;

// Type aliases to match Go implementation
pub type NostrEvent = Event;

// Common error types
#[derive(Debug, thiserror::Error)]
pub enum NostrError {
    #[error("Database error: {0}")]
    Network(String),
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("Signer error: {0}")]
    Signer(String),
    #[error("Relay error: {0}")]
    Relay(#[from] relays::types::RelayError),
    #[error("Types error: {0}")]
    Types(#[from] types::TypesError),
    #[error("Parser error: {0}")]
    Parser(#[from] parser::ParserError),
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("Other error: {0}")]
    Other(String),
}

impl From<gloo_net::Error> for NostrError {
    fn from(err: gloo_net::Error) -> Self {
        NostrError::Http(err.to_string())
    }
}

impl Into<JsValue> for NostrError {
    fn into(self) -> JsValue {
        JsValue::from_str(&self.to_string())
    }
}

// Common result type
pub type NostrResult<T> = Result<T, NostrError>;

// Worker implementation
use js_sys::Uint8Array;
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Once},
};
use tracing::info;

use crate::{generated::nostr::fb, types::nostr::Template, utils::js_interop::post_worker_message};

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
    network_manager: NetworkManager,
    signer_manager: Arc<SignerManager>,
}

#[wasm_bindgen]
impl NostrClient {
    #[wasm_bindgen(constructor)]
    pub async fn new(
        db_ring: SharedArrayBuffer,
        cache_request: SharedArrayBuffer, // ws request
        cache_response: SharedArrayBuffer,
        ws_response: SharedArrayBuffer, // ws response
    ) -> Self {
        telemetry::init(tracing::Level::ERROR);

        info!("Initializing NostrClient...");

        let signer_manager = Arc::new(SignerManager::new());

        let parser = Arc::new(Parser::new_with_signer(signer_manager.clone()));

        let network_manager = NetworkManager::new(
            parser,
            Rc::new(RefCell::new(
                SabRing::new(cache_request).expect("Failed to create SabRing for ws_request"),
            )),
            Rc::new(RefCell::new(
                SabRing::new(cache_response).expect("Failed to create SabRing for cache_response"),
            )),
            Rc::new(RefCell::new(
                SabRing::new(ws_response).expect("Failed to create SabRing for ws_response"),
            )),
            Rc::new(RefCell::new(
                SabRing::new(db_ring).expect("Failed to create SabRing for ws_response"),
            )),
        );

        info!("NostrClient components initialized");

        Self {
            network_manager,
            signer_manager,
        }
    }

    pub async fn close_subscription(&self, subscription_id: String) -> Result<(), JsValue> {
        self.network_manager
            .close_subscription(subscription_id)
            .await
            .map_err(|e| JsValue::from_str(&format!("Failed to close subscription: {}", e)))
    }

    fn sign_event(&self, template: Template) -> Result<(), JsValue> {
        // Sign the event using the signer manager
        let signed_event = self
            .signer_manager
            .sign_event(&template)
            .map_err(|e| JsValue::from_str(&format!("Failed to sign event: {}", e)))?;
        let json_str = signed_event.as_json();
        let js_value = JsValue::from_str(&json_str);
        info!("Event signed and ready for posting: {}", json_str);
        post_worker_message(&js_value);
        Ok(())
    }

    pub fn get_public_key(&self) -> Result<(), JsValue> {
        // Get the public key from signer manager
        let pubkey = self
            .signer_manager
            .get_public_key()
            .map_err(|e| JsValue::from_str(&format!("Failed to get public key: {}", e)))?;

        // Create FlatBuffer message
        let mut builder = FlatBufferBuilder::new();

        // Create the pubkey string
        let pubkey_offset = builder.create_string(&pubkey);

        // Create PubKeyPayload
        let pubkey_payload = fb::Pubkey::create(
            &mut builder,
            &fb::PubkeyArgs {
                pubkey: Some(pubkey_offset),
            },
        );

        // Create SignerMessage wrapper
        let signer_msg = fb::WorkerMessage::create(
            &mut builder,
            &fb::WorkerMessageArgs {
                sub_id: None,
                url: None,
                type_: fb::MessageType::Pubkey,
                content_type: fb::Message::Pubkey,
                content: Some(pubkey_payload.as_union_value()),
            },
        );

        // Finish the buffer
        builder.finish(signer_msg, None);
        let data = builder.finished_data().to_vec();

        // Send the message
        let uint8_array = Uint8Array::new_with_length(data.len() as u32);
        uint8_array.copy_from(&data);
        post_worker_message(&uint8_array.into());

        Ok(())
    }

    pub async fn handle_message(&self, message_obj: &JsValue) -> Result<(), JsValue> {
        // Extract serialized message and optional SharedArrayBuffer
        let (message_bytes, shared_buffer) = if message_obj.is_instance_of::<Uint8Array>() {
            // Legacy format - just Uint8Array (check this first to avoid it being caught as an Object)
            let uint8_array: Uint8Array = message_obj.clone().dyn_into()?;
            (uint8_array.to_vec(), None)
        } else if let Some(obj) = message_obj.dyn_ref::<js_sys::Object>() {
            // New format with serializedMessage and sharedBuffer
            if js_sys::Reflect::has(obj, &JsValue::from_str("serializedMessage")).unwrap_or(false) {
                let serialized_msg =
                    js_sys::Reflect::get(obj, &JsValue::from_str("serializedMessage"))?;
                let message_uint8 = js_sys::Uint8Array::from(serialized_msg);
                let mut message_bytes = vec![0u8; message_uint8.length() as usize];
                message_uint8.copy_to(&mut message_bytes);

                // SharedArrayBuffer is optional - only Subscribe and Publish require it
                let shared_buffer = if js_sys::Reflect::has(obj, &JsValue::from_str("sharedBuffer"))
                    .unwrap_or(false)
                {
                    let buffer = js_sys::Reflect::get(obj, &JsValue::from_str("sharedBuffer"))?;
                    Some(
                        buffer
                            .dyn_into::<js_sys::SharedArrayBuffer>()
                            .map_err(|_| JsValue::from_str("Invalid SharedArrayBuffer"))?,
                    )
                } else {
                    None
                };

                (message_bytes, shared_buffer)
            } else {
                return Err(JsValue::from_str("Missing serializedMessage field"));
            }
        } else {
            return Err(JsValue::from_str("Invalid message format"));
        };

        // Decode FlatBuffer message
        let main_message = flatbuffers::root::<fb::MainMessage>(&message_bytes)
            .map_err(|e| JsValue::from_str(&format!("Failed to decode FlatBuffer: {:?}", e)))?;

        // Process based on message type
        match main_message.content_type() {
            fb::MainContent::Subscribe => {
                let subscribe = main_message.content_as_subscribe().unwrap();
                let shared_buffer = shared_buffer
                    .ok_or_else(|| JsValue::from_str("Subscribe requires SharedArrayBuffer"))?;

                let subscription_id = subscribe.subscription_id().to_string();

                // Convert FlatBuffer requests to your Request type
                let mut requests = Vec::new();
                let fb_requests = subscribe.requests();
                for i in 0..fb_requests.len() {
                    let fb_req = fb_requests.get(i);
                    requests.push(fb_req);
                }

                let fb_config = subscribe.config();

                self.network_manager
                    .open_subscription(subscription_id, shared_buffer, &requests, &fb_config)
                    .await
                    .map_err(|e| {
                        JsValue::from_str(&format!("Failed to open subscription: {}", e))
                    })?;
            }

            fb::MainContent::Unsubscribe => {
                let unsubscribe = main_message.content_as_unsubscribe().unwrap();
                let subscription_id = unsubscribe.subscription_id().to_string();
                self.network_manager
                    .close_subscription(subscription_id)
                    .await
                    .map_err(|e| JsValue::from_str(&format!("Failed to unsubscribe: {}", e)))?;
            }

            fb::MainContent::Publish => {
                let publish = main_message.content_as_publish().unwrap();
                let shared_buffer = shared_buffer
                    .ok_or_else(|| JsValue::from_str("Publish requires SharedArrayBuffer"))?;

                let publish_id = publish.publish_id().to_string();
                let template = Template::from_flatbuffer(&publish.template());
                let relays: Vec<String> = (0..publish.relays().len())
                    .map(|i| publish.relays().get(i).to_string())
                    .collect();

                self.network_manager
                    .publish_event(publish_id, &template, &relays, shared_buffer)
                    .await
                    .map_err(|e| JsValue::from_str(&format!("Failed to publish event: {}", e)))?;
            }

            fb::MainContent::SignEvent => {
                if let Some(sign_event) = main_message.content_as_sign_event() {
                    let template = Template::from_flatbuffer(&sign_event.template());
                    self.sign_event(template)?;
                } else {
                    return Err(JsValue::from_str("Invalid SignEvent message"));
                }
            }

            fb::MainContent::SetSigner => {
                let set_signer = main_message.content_as_set_signer().unwrap();
                match set_signer.signer_type_type() {
                    fb::SignerType::PrivateKey => {
                        let pk_type = set_signer.signer_type_as_private_key().unwrap();
                        self.signer_manager
                            .set_privatekey_signer(pk_type.private_key())
                            .map_err(|e| {
                                JsValue::from_str(&format!(
                                    "Failed to set private key signer: {}",
                                    e
                                ))
                            })?;
                    }
                    _ => {
                        // Handle other signer types here
                    }
                }

                // self.set_signer(set_signer)?;
            }

            fb::MainContent::GetPublicKey => {
                // GetPublicKey has no fields, just process it
                self.get_public_key()?;
            }

            _ => {
                return Err(JsValue::from_str("Empty message content"));
            }
        }

        Ok(())
    }
}
