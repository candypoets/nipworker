use serde_json::Value;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

use js_sys::SharedArrayBuffer;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Once;

mod connection;
mod connection_registry;
mod fb_utils;
mod generated;
mod sab_ring;
mod types;

use connection_registry::ConnectionRegistry;
use sab_ring::SabRing;

use crate::fb_utils::build_worker_message;

#[derive(serde::Deserialize)]
struct Envelope {
    relays: Vec<String>,
    frames: Vec<String>,
}

// Common macros
#[macro_export]
macro_rules! console_log {
    ($($t:tt)*) => {
        web_sys::console::log_1(&format_args!($($t)*).to_string().into());
    }
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
    // Own the rings behind Rc so tasks can hold them without borrowing `self`.
    ws_request: Rc<RefCell<SabRing>>,
    registry: ConnectionRegistry,
}

#[wasm_bindgen]
impl WSRust {
    /// new(inRings: SharedArrayBuffer[], outRings: SharedArrayBuffer[])
    #[wasm_bindgen(constructor)]
    pub fn new(
        ws_request: SharedArrayBuffer,
        ws_response: SharedArrayBuffer,
        status_ring: SharedArrayBuffer,
    ) -> Result<WSRust, JsValue> {
        setup_tracing();
        let ws_request = Rc::new(RefCell::new(SabRing::new(ws_request)?));
        let ws_response = Rc::new(RefCell::new(SabRing::new(ws_response)?));
        let status_ring = Rc::new(RefCell::new(SabRing::new(status_ring)?));
        let out_ring_clone = ws_response.clone();

        let writer = Rc::new(move |url: &str, sub_id: &str, raw: &str| {
            // Build a WorkerMessage FlatBuffer and write its bytes
            let mut fbb = flatbuffers::FlatBufferBuilder::new();
            let wm = build_worker_message(&mut fbb, sub_id, url, raw);
            fbb.finish(wm, None);
            let bytes = fbb.finished_data().to_vec();

            if !bytes.is_empty() {
                out_ring_clone.borrow_mut().write(&bytes);
            } else {
                // Failure log (error handling) - now compiles with Value import
                let raw_sub_id = if let Ok(parsed) = serde_json::from_str::<Value>(raw) {
                    if let Value::Array(arr) = parsed {
                        arr.get(1).and_then(|v| v.as_str()).map(|s| s.to_string())
                    } else {
                        None
                    }
                } else {
                    None
                };
                console_log!(
                    "Failed to serialize envelope for caller_sub_id='{}': raw_sub_id={:?}, raw='{}' (len={})",
                    sub_id, raw_sub_id, raw, raw.len()
                );
            }
        });

        // Wire status writer
        let status_cell = status_ring.clone();
        let status_writer = Rc::new(move |status: &str, url: &str| {
            let mut ring = status_cell.borrow_mut();
            write_status_line(&mut ring, status, url);
        });

        // Build registry and wire writer that routes by subId to the correct out ring
        let registry = ConnectionRegistry::new(writer, status_writer);

        Ok(WSRust {
            ws_request,
            registry,
        })
    }

    /// Start one loop per inRing that reads JSON envelopes and calls send_to_relays
    pub fn start(&self) {
        // Clone the Rc for the task so we don’t capture &self
        let ring_rc = self.ws_request.clone();
        let reg = self.registry.clone();

        spawn_local(async move {
            let mut sleep_ms: u32 = 16;
            let max_sleep_ms: u32 = 500;

            loop {
                let mut processed = 0usize;

                // Drain ring
                loop {
                    let bytes_opt = {
                        let mut ring = ring_rc.borrow_mut();
                        ring.read_next()
                    };

                    let Some(bytes) = bytes_opt else { break };

                    processed += 1;

                    if let Ok(env) = serde_json::from_slice::<Envelope>(&bytes) {
                        if !env.relays.is_empty() && !env.frames.is_empty() {
                            // Sync call to send_to_relays
                            if let Err(e) = reg.send_to_relays(&env.relays, &env.frames) {
                                tracing::error!("send_to_relays failed: {:?}", e);
                                // No retry; just log
                            }
                        }
                    }
                }

                if processed == 0 {
                    gloo_timers::future::TimeoutFuture::new(sleep_ms).await;
                    sleep_ms = (sleep_ms.saturating_mul(2)).min(max_sleep_ms);
                } else {
                    sleep_ms = 16;
                }
            }
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
