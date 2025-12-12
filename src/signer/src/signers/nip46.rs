use gloo_timers::future::TimeoutFuture;
use js_sys::Date;
use serde_json::json;
use shared::SabRing;
use std::cell::RefCell;
use std::rc::Rc;
use tracing::{info, warn};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

/// Configuration for a NIP-46 remote signer session (Nostr Connect).
#[derive(Clone, Debug)]
pub struct Nip46Config {
    /// Remote signer public key (hex, x-only)
    pub remote_signer_pubkey: String,
    /// Relays to use for the NIP-46 RPC traffic
    pub relays: Vec<String>,
    /// Prefer NIP-44 (v2) encryption. If false, attempt NIP-04 as fallback.
    pub use_nip44: bool,
    /// Optional app name or label to include as a tag in requests
    pub app_name: Option<String>,
}

/// Skeleton for a NIP-46 signer that communicates via dedicated SAB rings:
/// - ws_req: signer -> connections (Envelope { relays, frames })
/// - ws_resp: connections -> signer (WorkerMessage bytes; decoded in a later iteration)
///
/// This file provides the structure and lifecycle methods (start/open/close),
/// as well as stubs for `get_public_key` and `sign_event_value` RPC calls.
/// The actual encryption, event signing, and FB WorkerMessage parsing will be
/// wired in a subsequent pass.
pub struct Nip46Signer {
    cfg: Nip46Config,
    ws_req: Rc<RefCell<SabRing>>,
    ws_resp: Rc<RefCell<SabRing>>,

    /// Client ephemeral pubkey (hex). In the full implementation this is derived from an ephemeral keypair.
    client_pubkey_hex: String,
    /// Subscription id for the NIP-46 REQ. Namespaced to route back to the signer response ring in connections.
    sub_id: String,

    /// Guard to ensure the response pump is only spawned once.
    pump_started: Rc<std::cell::Cell<bool>>,
}

impl Nip46Signer {
    /// Create a new NIP-46 signer over the given SAB rings.
    ///
    /// Note: This uses placeholder client pubkey generation. Replace with a real ephemeral keypair.
    pub fn new(
        cfg: Nip46Config,
        ws_req: Rc<RefCell<SabRing>>,
        ws_resp: Rc<RefCell<SabRing>>,
    ) -> Self {
        let client_pubkey_hex = Self::placeholder_client_pubkey();
        let sub_id = format!("n46:{}", client_pubkey_hex);

        Self {
            cfg,
            ws_req,
            ws_resp,
            client_pubkey_hex,
            sub_id,
            pump_started: Rc::new(std::cell::Cell::new(false)),
        }
    }

    /// Start the NIP-46 session:
    /// - open the REQ subscription for kind 24133 events addressed to this client
    /// - spawn the background response pump (logs for now)
    pub fn start(&self) {
        self.open_req_subscription();
        self.spawn_pump_once();
        info!("[nip46] started (sub_id={})", self.sub_id);
    }

    /// Close the NIP-46 REQ subscription.
    pub fn close(&self) {
        self.send_close();
    }

    /// RPC: request the remote signer's public key.
    ///
    /// This is a stub and currently returns an error until the RPC machinery is fully implemented.
    pub async fn get_public_key(&self) -> Result<String, JsValue> {
        Err(JsValue::from_str(
            "NIP-46 get_public_key not implemented in skeleton",
        ))
    }

    /// RPC: ask the remote signer to sign the provided Template (as serde_json::Value).
    ///
    /// This is a stub and currently returns an error until the RPC machinery is fully implemented.
    pub async fn sign_event(&self, _template: &str) -> Result<serde_json::Value, JsValue> {
        Err(JsValue::from_str(
            "NIP-46 sign_event not implemented in skeleton",
        ))
    }

    // ------------------------
    // Transport/frame helpers
    // ------------------------

    /// Open a REQ with a filter for kind 24133 addressed to client_pubkey.
    fn open_req_subscription(&self) {
        let filter = json!({
            "kinds": [24133],
            "#p": [self.client_pubkey_hex],
            "since": Self::unix_time() - 10
        })
        .to_string();

        let frame = format!(r#"["REQ","{}",{}]"#, self.sub_id, filter);
        self.publish_frames(&[frame]);
        info!("[nip46] REQ opened (sub_id={})", self.sub_id);
    }

    /// Send CLOSE for the current sub_id.
    fn send_close(&self) {
        let frame = format!(r#"["CLOSE","{}"]"#, self.sub_id);
        self.publish_frames(&[frame]);
        info!("[nip46] CLOSE sent (sub_id={})", self.sub_id);
    }

    /// Publish an EVENT containing a NIP-46 request (skeleton).
    ///
    /// In the full implementation:
    /// - Build JSON-RPC payload {"id": "...", "method": "...", "params": [...]}
    /// - Encrypt with NIP-44 (preferred) or NIP-04
    /// - Construct kind 24133 event with ["p", remote_pubkey], optional ["client", app_name]
    /// - Sign the event with the ephemeral client key
    /// - Publish as ["EVENT", {event_json}]
    #[allow(unused)]
    fn publish_nip46_event(&self, encrypted_content: &str) {
        let mut tags = vec![vec!["p".to_string(), self.cfg.remote_signer_pubkey.clone()]];
        if let Some(app) = &self.cfg.app_name {
            tags.push(vec!["client".to_string(), app.clone()]);
        }

        // Note: This event is intentionally incomplete (no id/sig). It's a skeleton to be filled in.
        let event = json!({
            "kind": 24133u32,
            "created_at": Self::unix_time(),
            "content": encrypted_content,
            "tags": tags,
            "pubkey": self.client_pubkey_hex,
        });

        let frame = format!(r#"["EVENT",{}]"#, event.to_string());
        self.publish_frames(&[frame]);
    }

    /// Publish one or more frames using the connections Envelope format:
    /// { "relays": [...], "frames": [...] }
    fn publish_frames(&self, frames: &[String]) {
        let env = json!({
            "relays": self.cfg.relays,
            "frames": frames,
        });

        match serde_json::to_vec(&env) {
            Ok(buf) => {
                let ok = self.ws_req.borrow_mut().write(&buf);
                if !ok {
                    warn!(
                        "[nip46] ws_req ring full, dropped {} frame(s)",
                        frames.len()
                    );
                }
            }
            Err(e) => warn!("[nip46] failed to serialize Envelope: {}", e),
        }
    }

    /// Spawn a single background pump to drain ws_response_signer (skeleton).
    /// For now, this only logs frames or binary sizes. Later this will:
    /// - decode fb::WorkerMessage
    /// - match sub_id
    /// - decrypt content
    /// - correlate JSON-RPC id -> resolve pending futures
    fn spawn_pump_once(&self) {
        if self.pump_started.get() {
            return;
        }
        self.pump_started.set(true);

        let ws_resp = self.ws_resp.clone();

        spawn_local(async move {
            let mut sleep_ms: u32 = 8;
            let max_sleep: u32 = 256;

            loop {
                let maybe = { ws_resp.borrow_mut().read_next() };

                if let Some(bytes) = maybe {
                    sleep_ms = 8;

                    if let Ok(s) = std::str::from_utf8(&bytes) {
                        let prev = preview(s, 160);
                        info!(
                            "[nip46] ws_response {} bytes (utf8 preview): {}",
                            bytes.len(),
                            prev
                        );
                    } else {
                        info!("[nip46] ws_response {} binary bytes", bytes.len());
                    }

                    continue;
                }

                TimeoutFuture::new(sleep_ms).await;
                sleep_ms = (sleep_ms * 2).min(max_sleep);
            }
        });

        info!("[nip46] response pump started");
    }

    // ----------------
    // Utility helpers
    // ----------------

    /// Placeholder client pubkey; replace with a real ephemeral keypair (x-only hex).
    fn placeholder_client_pubkey() -> String {
        // A short random-ish suffix to avoid identical sub_ids across reloads
        let t = Self::unix_time();
        format!("client_ephemeral_pubkey_{:x}", t)
    }

    /// Return Unix time in seconds (from JS Date.now()).
    fn unix_time() -> u32 {
        (Date::now() / 1000f64) as u32
    }
}

fn preview(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…(+{} bytes)", &s[..max], s.len() - max)
    }
}
