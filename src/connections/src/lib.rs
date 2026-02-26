use serde_json::Value;
use shared::{telemetry, Port};
use tracing::{error, info, warn};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::MessagePort;

use std::cell::RefCell;
use std::rc::Rc;

use futures::channel::mpsc;
use futures::select;
use futures::FutureExt;
use futures::StreamExt;

mod connection;
mod connection_registry;
mod fb_utils;
mod types;

use connection_registry::ConnectionRegistry;

use crate::fb_utils::build_worker_message;

#[derive(serde::Deserialize)]
struct Envelope {
    relays: Vec<String>,
    frames: Vec<String>,
}

#[wasm_bindgen]
pub struct WSRust {
    registry: ConnectionRegistry,
}

#[wasm_bindgen]
impl WSRust {
    /// new(toMain: MessagePort, fromCache: MessagePort, toParser: MessagePort, fromCrypto: MessagePort, toCrypto: MessagePort)
    #[wasm_bindgen(constructor)]
    pub fn new(
        to_main: MessagePort,
        from_cache: MessagePort,
        to_parser: MessagePort,
        from_crypto: MessagePort,
        to_crypto: MessagePort,
    ) -> Result<WSRust, JsValue> {
        telemetry::init(tracing::Level::ERROR);

        info!("instanciating connections");

        // Create receivers from the MessagePorts
        let from_cache_rx = Port::from_receiver(from_cache);
        let from_crypto_rx = Port::from_receiver(from_crypto);

        // Wrap the to_parser and to_crypto ports for sending
        let to_parser_port = Port::new(to_parser);
        let to_crypto_port = Rc::new(RefCell::new(Port::new(to_crypto))); // Used for auth signing requests and NIP-46

        // Clone for the writer closure
        let to_crypto_for_writer = to_crypto_port.clone();

        // Wire status writer - sends status updates via MessagePort to main thread
        let to_main_for_status = to_main.clone();
        let status_writer = Rc::new(move |status: &str, url: &str| {
            let msg = serde_json::json!({
                "type": "relay:status",
                "status": status,
                "url": url
            });
            let _ =
                to_main_for_status.post_message(&wasm_bindgen::JsValue::from_str(&msg.to_string()));
        });

        // Create the writer closure that routes messages based on sub_id
        // NIP-46 messages (sub_id starting with "n46:") go to crypto worker
        // All other messages go to parser worker
        let writer = Rc::new(move |url: &str, sub_id: &str, raw: &str| {
            // Build a WorkerMessage FlatBuffer and send its bytes through the appropriate port
            let mut fbb = flatbuffers::FlatBufferBuilder::new();
            let wm = build_worker_message(&mut fbb, sub_id, url, raw);
            fbb.finish(wm, None);
            let bytes = fbb.finished_data();

            if bytes.is_empty() {
                // Failure log (error handling)
                let raw_sub_id = if let Ok(parsed) = serde_json::from_str::<Value>(raw) {
                    if let Value::Array(arr) = parsed {
                        arr.get(1).and_then(|v| v.as_str()).map(|s| s.to_string())
                    } else {
                        None
                    }
                } else {
                    None
                };
                warn!(
                    "Failed to serialize envelope for caller_sub_id='{}': raw_sub_id={:?}, raw='{}' (len={})",
                    sub_id, raw_sub_id, raw, raw.len()
                );
                return;
            }

            // Route NIP-46 messages to crypto worker, others to parser
            let is_nip46 = sub_id.starts_with("n46:");
            let result = if is_nip46 {
                info!(
                    "[connections] Routing NIP-46 message (sub_id={}) to crypto worker",
                    sub_id
                );
                to_crypto_for_writer.borrow().send(bytes)
            } else {
                to_parser_port.send(bytes)
            };

            if let Err(e) = result {
                warn!(
                    "[connections] Failed to send message to {} port: {:?}",
                    if is_nip46 { "crypto" } else { "parser" },
                    e
                );
            }
        });

        // Build registry and wire writer
        let registry = ConnectionRegistry::new(writer, status_writer, to_crypto_port);

        let connections = WSRust { registry };

        connections.start(from_cache_rx, from_crypto_rx);

        Ok(connections)
    }

    pub fn close(&self, sub_id: &str) {
        let reg = self.registry.clone();
        reg.close_all(sub_id);
    }

    /// Start the async loop that reads from both receivers using select!
    fn start(
        &self,
        mut from_cache_rx: mpsc::Receiver<Vec<u8>>,
        mut from_crypto_rx: mpsc::Receiver<Vec<u8>>,
    ) {
        info!("Starting WebSocket server");
        let reg = self.registry.clone();

        spawn_local(async move {
            loop {
                // Use select! to race between the two receivers
                let bytes: Option<Vec<u8>> = select! {
                    bytes_opt = from_cache_rx.next().fuse() => {
                        if bytes_opt.is_some() {
                            info!("[connections] Received message from cache port");
                        } else {
                            info!("[connections] Cache channel closed");
                        }
                        bytes_opt
                    }
                    bytes_opt = from_crypto_rx.next().fuse() => {
                        if bytes_opt.is_some() {
                            info!("[connections] Received message from crypto port (NIP-46 or Auth)");
                        } else {
                            info!("[connections] Crypto channel closed");
                        }
                        bytes_opt
                    }
                };

                // Break if either channel closed
                let bytes = match bytes {
                    Some(b) => b,
                    None => break,
                };

                // Try to parse as Envelope (from cache)
                if let Ok(env) = serde_json::from_slice::<Envelope>(&bytes) {
                    if !env.relays.is_empty() && !env.frames.is_empty() {
                        info!(
                            "[connections] Sending {} frames to {} relays",
                            env.frames.len(),
                            env.relays.len()
                        );
                        if let Err(e) = reg.send_to_relays(&env.relays, &env.frames) {
                            error!("send_to_relays failed: {:?}", e);
                        }
                    } else if !env.frames.is_empty() && env.relays.is_empty() {
                        error!("[connections] CRITICAL: Envelope has frames but no relays - message dropped");
                    } else {
                        warn!("[connections] Envelope has no relays or frames");
                    }
                    continue;
                }

                // Try to parse as SignerResponse (from crypto for AuthEvent)
                if let Ok(signer_resp) =
                    flatbuffers::root::<shared::generated::nostr::fb::SignerResponse>(&bytes)
                {
                    let rid = signer_resp.request_id();
                    info!(
                        "[connections] Parsed SignerResponse from crypto: request_id={}",
                        rid
                    );
                    // Check if this is an AuthEvent response (high bit set)
                    if rid >= 0x8000_0000_0000_0000 {
                        info!("[connections][AUTH] Response detected (request_id={})", rid);
                        if let Some(result) = signer_resp.result() {
                            info!("[connections][AUTH] Has result (len={}), calling handle_auth_response", result.len());
                            info!(
                                "[connections][AUTH] Result preview: {}",
                                &result[..result.len().min(200)]
                            );
                            reg.handle_auth_response(result);
                            info!("[connections][AUTH] handle_auth_response returned");
                        } else if let Some(err) = signer_resp.error() {
                            error!("[connections][AUTH] Signing failed: {}", err);
                        } else {
                            error!("[connections][AUTH] Response has neither result nor error");
                        }
                    } else {
                        // Other signer responses - log but don't process here
                        info!(
                            "[connections] Received non-auth SignerResponse (request_id={})",
                            rid
                        );
                    }
                    continue;
                }

                warn!("[connections] Failed to parse message from bytes");
            }

            info!("[connections] Message processing loop ended");
        });
    }
}

/// Utility functions for the relay module
pub mod utils {
    use crate::types::RelayError;

    // const BLACKLISTED_RELAYS: &[&str] = &["wheat.happytavern.co"];

    pub fn extract_first_three<'a>(text: &'a str) -> Option<[Option<&'a str>; 3]> {
        let bytes = text.as_bytes();
        if bytes.first()? != &b'[' {
            return None;
        }
        let mut idx = 1; // skip first '['
        let mut results: [Option<&str>; 3] = [None, None, None];
        let mut found = 0;

        while found < 3 && idx < bytes.len() {
            // skip whitespace and commas
            while idx < bytes.len()
                && (bytes[idx] == b' '
                    || bytes[idx] == b'\n'
                    || bytes[idx] == b'\r'
                    || bytes[idx] == b',')
            {
                idx += 1;
            }

            if idx >= bytes.len() || bytes[idx] == b']' {
                break;
            }

            let start = idx;

            if bytes[idx] == b'"' {
                // String element
                idx += 1;
                while idx < bytes.len() {
                    match bytes[idx] {
                        b'\\' => idx += 2, // skip escaped char
                        b'"' => {
                            let s = &text[start..=idx];
                            results[found] = Some(s);
                            idx += 1;
                            break;
                        }
                        _ => idx += 1,
                    }
                }
            } else if bytes[idx] == b'{' {
                // Object element — find matching closing '}'
                let mut brace_count = 1;
                idx += 1;
                while idx < bytes.len() && brace_count > 0 {
                    match bytes[idx] {
                        b'{' => brace_count += 1,
                        b'}' => brace_count -= 1,
                        b'"' => {
                            // skip string inside object
                            idx += 1;
                            while idx < bytes.len() {
                                if bytes[idx] == b'\\' {
                                    idx += 2;
                                    continue;
                                }
                                if bytes[idx] == b'"' {
                                    break;
                                }
                                idx += 1;
                            }
                        }
                        _ => {}
                    }
                    idx += 1;
                }
                let s = &text[start..idx];
                results[found] = Some(s);
            } else {
                // Primitive (number, bool, null)
                while idx < bytes.len() && bytes[idx] != b',' && bytes[idx] != b']' {
                    idx += 1;
                }
                let s = text[start..idx].trim();
                results[found] = Some(s);
            }

            found += 1;
        }

        Some(results)
    }

    /// Validate relay URL format
    pub fn validate_relay_url(url: &str) -> Result<(), RelayError> {
        if url.is_empty() {
            return Err(RelayError::InvalidUrl("URL cannot be empty".to_string()));
        }

        let normalized_url = url.trim().to_lowercase();
        // for &blacklisted in BLACKLISTED_RELAYS {
        //     if normalized_url.contains(blacklisted) {
        //         return Err(RelayError::InvalidUrl(format!(
        //             "Relay URL is blacklisted: {}",
        //             url
        //         )));
        //     }
        // }

        if !url.starts_with("ws://") && !url.starts_with("wss://") {
            return Err(RelayError::InvalidUrl(
                "URL must start with ws:// or wss://".to_string(),
            ));
        }

        Ok(())
    }

    /// Normalize relay URL (remove trailing slash, convert to lowercase)
    pub fn normalize_relay_url(url: &str) -> String {
        let mut normalized = url.trim().to_lowercase();
        if normalized.ends_with('/') && normalized.len() > 1 {
            normalized.pop();
        }
        normalized
    }
}
