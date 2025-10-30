use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

use js_sys::{Array, SharedArrayBuffer};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Once};

mod connection;
mod connection_registry;
mod sab_ring;
mod types;

use connection_registry::ConnectionRegistry;
use sab_ring::SabRing;

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
            .with_max_level(tracing::Level::ERROR)
            .try_init();

        console_log!("Tracing subscriber initialized for Web Worker");
    });
}

// Same hashing logic as TS (hashSubId)
fn hash_sub_id(sub_id: &str, rings_len: usize) -> usize {
    if rings_len == 0 {
        return 0;
    }
    let target = if let Some(idx) = sub_id.find('_') {
        &sub_id[idx + 1..]
    } else {
        sub_id
    };
    let mut hash: i32 = 0;
    for ch in target.chars() {
        hash = (hash << 5) - hash + (ch as i32);
    }
    hash.unsigned_abs() as usize % rings_len
}

fn write_worker_line(out_ring: &mut SabRing, url: &str, raw: &str) {
    // [u16_be url_len][url][u32_be raw_len][raw]
    let url_b = url.as_bytes();
    let raw_b = raw.as_bytes();
    let total = 2 + url_b.len() + 4 + raw_b.len();
    let mut buf = vec![0u8; total];
    buf[0..2].copy_from_slice(&(url_b.len() as u16).to_be_bytes());
    let mut o = 2usize;
    buf[o..o + url_b.len()].copy_from_slice(url_b);
    o += url_b.len();
    buf[o..o + 4].copy_from_slice(&(raw_b.len() as u32).to_be_bytes());
    o += 4;
    buf[o..].copy_from_slice(raw_b);
    out_ring.write(&buf);
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
    in_rings: Vec<Rc<RefCell<SabRing>>>,
    out_rings: Vec<Rc<RefCell<SabRing>>>,
    status_ring: Rc<RefCell<SabRing>>,
    registry: ConnectionRegistry,
}

#[wasm_bindgen]
impl WSRust {
    /// new(inRings: SharedArrayBuffer[], outRings: SharedArrayBuffer[])
    #[wasm_bindgen(constructor)]
    pub fn new(
        in_rings: Array,
        out_rings: Array,
        status_ring: SharedArrayBuffer,
    ) -> Result<WSRust, JsValue> {
        setup_tracing();
        let mut in_vec: Vec<Rc<RefCell<SabRing>>> = Vec::new();
        for v in in_rings.iter() {
            let sab: SharedArrayBuffer = v.dyn_into()?;
            in_vec.push(Rc::new(RefCell::new(SabRing::new(sab)?)));
        }
        let mut out_vec: Vec<Rc<RefCell<SabRing>>> = Vec::new();
        for v in out_rings.iter() {
            let sab: SharedArrayBuffer = v.dyn_into()?;
            out_vec.push(Rc::new(RefCell::new(SabRing::new(sab)?)));
        }
        let status_ring = Rc::new(RefCell::new(SabRing::new(status_ring)?));

        // Build registry and wire writer that routes by subId to the correct out ring
        let mut registry = ConnectionRegistry::new();
        let out_vec_for_writer = out_vec.clone();
        let writer = Rc::new(move |url: &str, sub_id: &str, raw: &str| {
            let idx = hash_sub_id(sub_id, out_vec_for_writer.len());
            if let Some(cell) = out_vec_for_writer.get(idx) {
                let mut ring = cell.borrow_mut();
                write_worker_line(&mut ring, url, raw);
            }
        });
        registry.set_out_writer(writer);

        // Wire status writer
        let status_cell = status_ring.clone();
        let status_writer = Rc::new(move |status: &str, url: &str| {
            let mut ring = status_cell.borrow_mut();
            write_status_line(&mut ring, status, url);
        });
        registry.set_status_writer(status_writer);

        Ok(WSRust {
            in_rings: in_vec,
            out_rings: out_vec,
            status_ring,
            registry,
        })
    }

    /// Start one loop per inRing that reads JSON envelopes and calls send_to_relays
    pub fn start(&self) {
        let max_successes = 5usize;
        let max_concurrency = 5usize;

        // Clone the Rc for each task so we don’t capture &self
        for ring_rc in self.in_rings.iter().cloned() {
            let reg = self.registry.clone();

            spawn_local(async move {
                // Inside the spawn_local(async move { ... }) block
                let mut sleep_ms: u32 = 16; // base delay
                let max_sleep_ms: u32 = 500; // cap the backoff

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
                                let reg2 = reg.clone();
                                let relays = env.relays;
                                let frames = env.frames;
                                spawn_local(async move {
                                    reg2.send_to_relays(
                                        relays,
                                        frames,
                                        max_successes,
                                        max_concurrency,
                                    )
                                    .await;
                                });
                            }
                        }
                    }

                    if processed == 0 {
                        gloo_timers::future::TimeoutFuture::new(sleep_ms).await;
                        sleep_ms = (sleep_ms.saturating_mul(2)).min(max_sleep_ms);
                    // backoff
                    } else {
                        sleep_ms = 16; // reset on activity
                    }
                }
            });
        }
    }
}

/// Utility functions for the relay module
pub mod utils {
    use crate::types::RelayError;

    const BLACKLISTED_RELAYS: &[&str] = &["wheat.happytavern.co"];

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
        for &blacklisted in BLACKLISTED_RELAYS {
            if normalized_url.contains(blacklisted) {
                return Err(RelayError::InvalidUrl(format!(
                    "Relay URL is blacklisted: {}",
                    url
                )));
            }
        }

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
