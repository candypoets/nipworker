#![allow(async_fn_in_trait)]

use flatbuffers::FlatBufferBuilder;
use shared::{
    init_with_component, telemetry,
    types::{nostr::Template, Event, ParserError, TypesError},
    Port,
};
use wasm_bindgen::prelude::*;
use web_sys::MessagePort;

pub mod crypto_client;
pub mod network;
pub mod parser;
pub mod pipeline;
pub mod relays;
pub mod types;
pub mod utils;

pub use crypto_client::CryptoClient;

// Re-export key types for external use
pub use network::NetworkManager;
pub use parser::Parser;
// pub use crypto::{PrivateKeySigner, SignerInterface, SignerManager, SignerManagerInterface};

// Type aliases to match Go implementation
pub type NostrEvent = Event;

// Common error types
#[derive(Debug, thiserror::Error)]
pub enum NostrError {
    #[error("Database error: {0}")]
    Network(String),
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("Crypto error: {0}")]
    Crypto(String),
    #[error("Relay error: {0}")]
    Relay(#[from] relays::types::RelayError),
    #[error("Types error: {0}")]
    Types(#[from] TypesError),
    #[error("Parser error: {0}")]
    Parser(#[from] ParserError),
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

use crate::utils::js_interop::post_worker_message;
use shared::generated::nostr::fb;

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
    crypto_client: Arc<CryptoClient>,
    parser: Arc<Parser>,
}

#[wasm_bindgen]
impl NostrClient {
    #[wasm_bindgen(constructor)]
    pub async fn new(
        from_connections: MessagePort,
        to_cache: MessagePort,
        from_cache: MessagePort,
        to_crypto: MessagePort,
        from_crypto: MessagePort,
        to_main: MessagePort,
    ) -> Self {
        init_with_component(tracing::Level::INFO, "PARSER");

        info!("Initializing NostrClient with MessageChannel...");

        // Create receivers from MessagePorts for network messages
        let from_connections_rx = Port::from_receiver(from_connections);
        let from_cache_rx = Port::from_receiver(from_cache);

        // Wrap to_cache port for sending cache requests
        let to_cache_port = Rc::new(RefCell::new(Port::new(to_cache)));

        // let signer_manager = Arc::new(SignerManager::new());
        let crypto_client = Arc::new(
            CryptoClient::new(to_crypto, from_crypto).expect("Failed to initialize signer client"),
        );

        let parser = Arc::new(Parser::new(crypto_client.clone()));

        let network_manager = NetworkManager::new(
            parser.clone(),
            to_cache_port,
            from_connections_rx,
            from_cache_rx,
            crypto_client.clone(),
            to_main,
        );

        info!("NostrClient components initialized");

        Self {
            network_manager,
            crypto_client,
            parser,
        }
    }

    pub async fn close_subscription(&self, subscription_id: String) -> Result<(), JsValue> {
        self.network_manager
            .close_subscription(subscription_id)
            .await
            .map_err(|e| JsValue::from_str(&format!("Failed to close subscription: {}", e)))
    }

    pub async fn get_public_key(&self) -> Result<(), JsValue> {
        // Get the public key from signer manager
        let pubkey = self
            .crypto_client
            .get_public_key()
            .await // Await the future returned by get_public_key
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
        let mut builder = flatbuffers::FlatBufferBuilder::new();
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
        // Extract serialized message
        let message_bytes = if message_obj.is_instance_of::<Uint8Array>() {
            // Legacy format - just Uint8Array (check this first to avoid it being caught as an Object)
            let uint8_array: Uint8Array = message_obj.clone().dyn_into()?;
            uint8_array.to_vec()
        } else if let Some(obj) = message_obj.dyn_ref::<js_sys::Object>() {
            // New format with serializedMessage
            if js_sys::Reflect::has(obj, &JsValue::from_str("serializedMessage")).unwrap_or(false) {
                let serialized_msg =
                    js_sys::Reflect::get(obj, &JsValue::from_str("serializedMessage"))?;
                let message_uint8 = js_sys::Uint8Array::from(serialized_msg);
                let mut message_bytes = vec![0u8; message_uint8.length() as usize];
                message_uint8.copy_to(&mut message_bytes);
                message_bytes
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
                    .open_subscription(subscription_id, &requests, &fb_config)
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
                let publish_id = publish.publish_id().to_string();
                let template = Template::from_flatbuffer(&publish.template());
                let relays: Vec<String> = (0..publish.relays().len())
                    .map(|i| publish.relays().get(i).to_string())
                    .collect();

                info!(
                    "[parser] received Publish message: publish_id={}, kind={}, relays={:?}",
                    publish_id, template.kind, relays
                );

                self.network_manager
                    .publish_event(publish_id, &template, &relays)
                    .await
                    .map_err(|e| JsValue::from_str(&format!("Failed to publish event: {}", e)))?;

                info!("[parser] publish_event completed successfully");
            }

            fb::MainContent::SignEvent => {
                if let Some(sign_event) = main_message.content_as_sign_event() {
                    let template = Template::from_flatbuffer(&sign_event.template());
                    // self.sign_event(template)?;
                } else {
                    return Err(JsValue::from_str("Invalid SignEvent message"));
                }
            }

            // fb::MainContent::SetSigner => {
            //     let set_signer = main_message.content_as_set_signer().unwrap();
            //     match set_signer.signer_type_type() {
            //         fb::SignerType::PrivateKey => {
            //             let pk_type = set_signer.signer_type_as_private_key().unwrap();
            //             self.signer_manager
            //                 .set_privatekey_signer(pk_type.private_key())
            //                 .map_err(|e| {
            //                     JsValue::from_str(&format!(
            //                         "Failed to set private key signer: {}",
            //                         e
            //                     ))
            //                 })?;
            //         }
            //         _ => {
            //             // Handle other signer types here
            //         }
            //     }

            //     // self.set_signer(set_signer)?;
            // }
            // fb::MainContent::GetPublicKey => {
            //     // GetPublicKey has no fields, just process it
            //     self.get_public_key()?;
            // }
            _ => {
                return Err(JsValue::from_str("Empty message content"));
            }
        }

        Ok(())
    }
}
