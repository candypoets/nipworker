use gloo_timers::future::TimeoutFuture;
use shared::SabRing;
use std::{cell::RefCell, rc::Rc};
use tracing::{info, warn};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

/// ServiceLoop is the background driver for the signer:
/// - drains signer_service_request and writes signer_service_response
/// - can also observe ws_response_signer (connections -> signer) as needed
///
/// This is a minimal placeholder that proves the SAB pipes and structure.
/// Integrate FlatBuffers decode/encode and actual signer dispatch in a follow-up.
pub struct ServiceLoop {
    // Parser <-> Signer rings (SPSC)
    svc_req: Rc<RefCell<SabRing>>,
    svc_resp: Rc<RefCell<SabRing>>,

    // Signer <-> Connections rings (SPSC)
    ws_req: Rc<RefCell<SabRing>>,
    ws_resp: Rc<RefCell<SabRing>>,
}

impl ServiceLoop {
    /// Accepts SAB-backed rings. The `active` parameter is intentionally generic and unused
    /// here so the caller can pass any active signer state without us depending on its type.
    pub fn new<T>(
        svc_req: Rc<RefCell<SabRing>>,
        svc_resp: Rc<RefCell<SabRing>>,
        ws_req: Rc<RefCell<SabRing>>,
        ws_resp: Rc<RefCell<SabRing>>,
        _active: T,
    ) -> Self {
        Self {
            svc_req,
            svc_resp,
            ws_req,
            ws_resp,
        }
    }

    /// Spawns the service loops.
    /// - service loop: echoes back any incoming payload to prove the pipe
    /// - nip46 pump: logs inbound frames from connections (placeholder)
    pub fn start(&mut self) {
        self.spawn_service_loop();
        self.spawn_nip46_pump();
        info!("[signer][service] loops started");
    }

    fn spawn_service_loop(&self) {
        let svc_req = self.svc_req.clone();
        let svc_resp = self.svc_resp.clone();

        spawn_local(async move {
            let mut sleep_ms: u32 = 8;
            let max_sleep: u32 = 256;

            loop {
                let maybe = { svc_req.borrow_mut().read_next() };

                if let Some(bytes) = maybe {
                    sleep_ms = 8;

                    // Placeholder behavior:
                    // - In the final version, decode FB::SignerRequest, dispatch to active signer,
                    //   and encode FB::SignerResponse.
                    // - For now, we echo the payload back so the caller can validate the pipe.
                    if !bytes.is_empty() {
                        let ok = svc_resp.borrow_mut().write(&bytes);
                        if !ok {
                            warn!("[signer][service] svc_resp ring full, response dropped");
                        }
                    } else {
                        let fallback = br#"{"ok":true}"#;
                        let ok = svc_resp.borrow_mut().write(fallback);
                        if !ok {
                            warn!("[signer][service] svc_resp ring full, fallback dropped");
                        }
                    }

                    continue;
                }

                TimeoutFuture::new(sleep_ms).await;
                sleep_ms = (sleep_ms * 2).min(max_sleep);
            }
        });

        info!("[signer][service] service loop spawned");
    }

    fn spawn_nip46_pump(&self) {
        let ws_resp = self.ws_resp.clone();

        spawn_local(async move {
            let mut sleep_ms: u32 = 8;
            let max_sleep: u32 = 256;

            loop {
                let maybe = { ws_resp.borrow_mut().read_next() };

                if let Some(bytes) = maybe {
                    sleep_ms = 8;

                    // Placeholder: In the complete version, decode fb::WorkerMessage,
                    // filter by sub_id, decrypt content (NIP-44/04), correlate by RPC id.
                    // Here we just log a short preview.
                    if let Ok(s) = std::str::from_utf8(&bytes) {
                        let prev = preview(s, 160);
                        info!(
                            "[signer][nip46] ws_response {} bytes: {}",
                            bytes.len(),
                            prev
                        );
                    } else {
                        info!("[signer][nip46] ws_response {} binary bytes", bytes.len());
                    }

                    continue;
                }

                TimeoutFuture::new(sleep_ms).await;
                sleep_ms = (sleep_ms * 2).min(max_sleep);
            }
        });

        info!("[signer][service] NIP-46 pump spawned");
    }

    #[allow(unused)]
    fn publish_frames(&self, relays: &[String], frames: &[String]) {
        // Helper for sending Envelope { relays, frames } to connections via ws_request_signer.
        // connections expects JSON with these two fields (consistent with current Envelope).
        let env = serde_json::json!({ "relays": relays, "frames": frames });
        if let Ok(buf) = serde_json::to_vec(&env) {
            let ok = self.ws_req.borrow_mut().write(&buf);
            if !ok {
                warn!("[signer][service] ws_req ring full, frame dropped");
            }
        }
    }
}

// ----------------------
// Placeholder submodules
// ----------------------

/// Placeholder NIP-07 module (browser signer via window.nostr)
pub mod nip07 {
    use super::*;
    use js_sys::{Object, Promise, Reflect};
    use wasm_bindgen::JsCast;

    pub struct Nip07Signer;

    impl Nip07Signer {
        pub fn new() -> Self {
            Self
        }

        pub async fn get_public_key(&self) -> Result<String, JsValue> {
            let win = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
            let nostr = Reflect::get(&win, &JsValue::from_str("nostr"))
                .map_err(|_| JsValue::from_str("window.nostr missing"))?;
            let get_pk = Reflect::get(&nostr, &JsValue::from_str("getPublicKey"))
                .map_err(|_| JsValue::from_str("nostr.getPublicKey missing"))?
                .dyn_into::<js_sys::Function>()
                .map_err(|_| JsValue::from_str("getPublicKey not a function"))?;
            let p = get_pk
                .call0(&nostr)
                .map_err(|_| JsValue::from_str("getPublicKey call failed"))?;
            let js = wasm_bindgen_futures::JsFuture::from(
                p.dyn_into::<Promise>()
                    .map_err(|_| JsValue::from_str("not a Promise"))?,
            )
            .await
            .map_err(|_| JsValue::from_str("getPublicKey rejected"))?;
            js.as_string()
                .ok_or_else(|| JsValue::from_str("invalid pk"))
        }

        pub async fn sign_event_json(&self, template_json: &str) -> Result<String, JsValue> {
            let tmpl_val: serde_json::Value = serde_json::from_str(template_json)
                .map_err(|e| JsValue::from_str(&format!("invalid template: {e}")))?;
            let win = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
            let nostr = Reflect::get(&win, &JsValue::from_str("nostr"))
                .map_err(|_| JsValue::from_str("window.nostr missing"))?;
            let sign_fn = Reflect::get(&nostr, &JsValue::from_str("signEvent"))
                .map_err(|_| JsValue::from_str("nostr.signEvent missing"))?
                .dyn_into::<js_sys::Function>()
                .map_err(|_| JsValue::from_str("signEvent not a function"))?;
            let js_evt = serde_wasm_bindgen::to_value(&tmpl_val)
                .map_err(|e| JsValue::from_str(&format!("serde_wasm_bindgen: {e}")))?;
            let p = sign_fn
                .call1(&nostr, &js_evt)
                .map_err(|_| JsValue::from_str("signEvent call failed"))?;
            let signed = wasm_bindgen_futures::JsFuture::from(
                p.dyn_into::<Promise>()
                    .map_err(|_| JsValue::from_str("not a Promise"))?,
            )
            .await
            .map_err(|_| JsValue::from_str("signEvent rejected"))?;
            let out: serde_json::Value = serde_wasm_bindgen::from_value(signed)
                .map_err(|e| JsValue::from_str(&e.to_string()))?;
            serde_json::to_string(&out).map_err(|e| JsValue::from_str(&e.to_string()))
        }
    }
}

/// Placeholder NIP-46 module (remote signer over relays)
pub mod nip46 {
    use super::*;

    #[derive(Clone)]
    pub struct Config {
        pub remote_signer_pubkey: String,
        pub relays: Vec<String>,
        pub use_nip44: bool,
        pub app_name: Option<String>,
    }

    /// Minimal skeleton for a NIP-46 signer.
    /// In the complete version, this will:
    /// - generate an ephemeral client keypair
    /// - open a REQ on relays and maintain a sub_id
    /// - encrypt/decrypt payloads (NIP-44 preferred, fallback NIP-04)
    /// - correlate JSON-RPC by id with a pending map
    pub struct Signer {
        cfg: Config,
        ws_req: Rc<RefCell<SabRing>>,
        ws_resp: Rc<RefCell<SabRing>>,
        sub_id: String,
    }

    impl Signer {
        pub fn new(cfg: Config, ws_req: SabRing, ws_resp: SabRing) -> Self {
            // Placeholder sub_id based on remote pubkey
            let sub_id = format!("n46:{}", cfg.remote_signer_pubkey);
            Self {
                cfg,
                ws_req: Rc::new(RefCell::new(ws_req)),
                ws_resp: Rc::new(RefCell::new(ws_resp)),
                sub_id,
            }
        }

        pub fn start(&self) {
            // In the full version, send a REQ with filter:
            // kinds: [24133], "#p": [client_pubkey]
            info!("[signer][nip46] start() placeholder (no-op)");
        }

        pub async fn get_public_key(&self) -> Result<String, JsValue> {
            Err(JsValue::from_str("NIP-46 get_public_key not implemented"))
        }

        pub async fn sign_event_json(&self, _template_json: &str) -> Result<String, JsValue> {
            Err(JsValue::from_str("NIP-46 sign_event not implemented"))
        }

        #[allow(unused)]
        fn publish_frames(&self, frames: &[String]) {
            let env = serde_json::json!({ "relays": self.cfg.relays, "frames": frames });
            if let Ok(buf) = serde_json::to_vec(&env) {
                let ok = self.ws_req.borrow_mut().write(&buf);
                if !ok {
                    warn!("[signer][nip46] ws_req ring full, frame dropped");
                }
            }
        }
    }
}

// -------------
// Small helpers
// -------------

fn preview(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let head = &s[..max];
        format!("{}…(+{} bytes)", head, s.len() - max)
    }
}
