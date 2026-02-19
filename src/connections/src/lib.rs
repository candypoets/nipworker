use serde_json::Value;
use shared::{telemetry, Port, SabRing};
use tracing::{error, info, warn};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::MessagePort;

use js_sys::SharedArrayBuffer;
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

fn write_status_line(status_ring: &mut SabRing, status: &str, url: &str) {
    // Simple ASCII: "status|url"
    // Frontend splits on the first '|' to parse.
    let mut line = String::with_capacity(status.len() + 1 + url.len());
    line.push_str(status);
    line.push('|');
    line.push_str(url);
    status_ring.write(line.as_bytes());
}

#[wasm_bindgen]
pub struct WSRust {
    registry: ConnectionRegistry,
}

#[wasm_bindgen]
impl WSRust {
    /// new(statusRing: SharedArrayBuffer, fromCache: MessagePort, toParser: MessagePort, fromCrypto: MessagePort, toCrypto: MessagePort)
    #[wasm_bindgen(constructor)]
    pub fn new(
        status_ring: SharedArrayBuffer,
        from_cache: MessagePort,
        to_parser: MessagePort,
        from_crypto: MessagePort,
        to_crypto: MessagePort,
    ) -> Result<WSRust, JsValue> {
        telemetry::init(tracing::Level::ERROR);

        info!("instanciating connections");
        let status_ring = Rc::new(RefCell::new(SabRing::new(status_ring)?));

        // Create receivers from the MessagePorts
        let from_cache_rx = Port::from_receiver(from_cache);
        let from_crypto_rx = Port::from_receiver(from_crypto);

        // Wrap the to_parser and to_crypto ports for sending
        let to_parser_port = Port::new(to_parser);
        let _to_crypto_port = Port::new(to_crypto); // Used for sending NIP-46 responses to crypto

        // Wire status writer
        let status_cell = status_ring.clone();
        let status_writer = Rc::new(move |status: &str, url: &str| {
            let mut ring = status_cell.borrow_mut();
            write_status_line(&mut ring, status, url);
        });

        // Create the writer closure that sends through the to_parser port
        let writer = Rc::new(move |url: &str, sub_id: &str, raw: &str| {
            // Build a WorkerMessage FlatBuffer and send its bytes through the port
            let mut fbb = flatbuffers::FlatBufferBuilder::new();
            let wm = build_worker_message(&mut fbb, sub_id, url, raw);
            fbb.finish(wm, None);
            let bytes = fbb.finished_data();

            if !bytes.is_empty() {
                if let Err(e) = to_parser_port.send(bytes) {
                    warn!("Failed to send message through to_parser port: {:?}", e);
                }
            } else {
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
            }
        });

        // Build registry and wire writer
        let registry = ConnectionRegistry::new(writer, status_writer);

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
                            info!("[connections] Received message from crypto port (NIP-46)");
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
                } else {
                    warn!("[connections] Failed to parse Envelope from ring bytes");
                }
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
