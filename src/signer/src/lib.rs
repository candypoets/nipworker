#![allow(clippy::needless_return)]
#![allow(clippy::should_implement_trait)]
#![allow(dead_code)]

use js_sys::{Array, SharedArrayBuffer, Uint8Array};
use shared::{telemetry, SabRing};
use tracing::{error, info, warn};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

use gloo_timers::future::TimeoutFuture;
use std::cell::RefCell;
use std::rc::Rc;

// expose client
mod client;
pub use client::SignerClient;

/// A minimal signer worker that:
/// - Wires four SAB rings:
///   - signer_service_request  (parser -> signer)
///   - signer_service_response (signer -> parser)
///   - ws_request_signer       (signer -> connections)
///   - ws_response_signer      (connections -> signer)
/// - Exposes direct methods for frontend (bypassing SAB).
///
/// NOTE:
/// - The service protocol (SignerRequest/SignerResponse) is intended to be FlatBuffers.
///   This initial version simply echoes back payloads to prove the pipe and will
///   be swapped to FlatBuffers once the schema is generated in `generated`.
/// - NIP-46 transport wiring is scaffolded. It publishes/consumes frames via the
///   dedicated ws SABs. Business logic will be implemented after schema/types land.
#[wasm_bindgen]
pub struct Signer {
    // Parser <-> Signer SABs (SPSC each)
    svc_req: Rc<RefCell<SabRing>>,
    svc_resp: Rc<RefCell<SabRing>>,

    // Signer <-> Connections SABs (SPSC each)
    ws_req: Rc<RefCell<SabRing>>,
    ws_resp: Rc<RefCell<SabRing>>,

    // Simple runtime state
    active: Rc<RefCell<ActiveSigner>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ActiveSigner {
    Unset,
    PrivateKey {
        secret: String,
    },
    Nip07,
    Nip46 {
        remote_pubkey_hex: String,
        relays: Vec<String>,
    },
}

#[wasm_bindgen]
impl Signer {
    /// new(signer_service_request, signer_service_response, ws_request_signer, ws_response_signer)
    #[wasm_bindgen(constructor)]
    pub fn new(
        signer_service_request: SharedArrayBuffer,
        signer_service_response: SharedArrayBuffer,
        ws_request_signer: SharedArrayBuffer,
        ws_response_signer: SharedArrayBuffer,
    ) -> Result<Signer, JsValue> {
        telemetry::init_with_component(tracing::Level::INFO, "signer");

        let svc_req = Rc::new(RefCell::new(SabRing::new(signer_service_request)?));
        let svc_resp = Rc::new(RefCell::new(SabRing::new(signer_service_response)?));
        let ws_req = Rc::new(RefCell::new(SabRing::new(ws_request_signer)?));
        let ws_resp = Rc::new(RefCell::new(SabRing::new(ws_response_signer)?));

        info!("[signer] initialized SAB rings");

        Ok(Signer {
            svc_req,
            svc_resp,
            ws_req,
            ws_resp,
            active: Rc::new(RefCell::new(ActiveSigner::Unset)),
        })
    }

    /// Starts:
    /// - the signer service loop (drains signer_service_request and writes signer_service_response)
    /// - the NIP-46 response pump (drains ws_response_signer)
    #[wasm_bindgen]
    pub fn start(&self) {
        self.start_service_loop();
        self.start_nip46_pump();
        info!("[signer] loops started");
    }

    // --------------------------
    // Direct methods (bypass SAB)
    // --------------------------

    /// Set a private key signer (hex or nsec). For now we don't perform validation here.
    #[wasm_bindgen(js_name = "setPrivateKey")]
    pub fn set_private_key(&self, secret: String) {
        *self.active.borrow_mut() = ActiveSigner::PrivateKey { secret };
        info!("[signer] active signer = PrivateKey");
    }

    /// Use NIP-07 (window.nostr). Assumes this signer runs in the main window context.
    #[wasm_bindgen(js_name = "setNip07")]
    pub fn set_nip07(&self) {
        *self.active.borrow_mut() = ActiveSigner::Nip07;
        info!("[signer] active signer = NIP-07");
    }

    /// Configure NIP-46 remote signer. Takes remote signer pubkey (hex) and relays.
    #[wasm_bindgen(js_name = "setNip46")]
    pub fn set_nip46(&self, remote_pubkey_hex: String, relays: Array) {
        let relays_vec: Vec<String> = relays.iter().filter_map(|v| v.as_string()).collect();
        *self.active.borrow_mut() = ActiveSigner::Nip46 {
            remote_pubkey_hex,
            relays: relays_vec,
        };
        info!("[signer] active signer = NIP-46 (configured)");
    }

    /// Direct path: get public key as a string. This is a placeholder until key plumbing lands.
    #[wasm_bindgen(js_name = "getPublicKeyDirect")]
    pub async fn get_public_key_direct(&self) -> Result<JsValue, JsValue> {
        match &*self.active.borrow() {
            ActiveSigner::PrivateKey { .. } => {
                // TODO: derive real pubkey using your crypto/types crate
                Ok(JsValue::from_str("pubkey-from-private-key-TODO"))
            }
            ActiveSigner::Nip07 => {
                // window.nostr.getPublicKey()
                let win = web_sys::window().ok_or_else(|| js_err("no window"))?;
                let nostr = js_get(&win, "nostr").ok_or_else(|| js_err("window.nostr missing"))?;
                let get_pk = js_get_fn(&nostr, "getPublicKey")
                    .ok_or_else(|| js_err("nostr.getPublicKey missing"))?;
                let promise = get_pk
                    .call0(&nostr)
                    .map_err(|_| js_err("nostr.getPublicKey call failed"))?;
                let pk = wasm_bindgen_futures::JsFuture::from(js_as_promise(promise)?)
                    .await
                    .map_err(|_| js_err("nostr.getPublicKey rejected"))?;
                Ok(pk)
            }
            ActiveSigner::Nip46 { .. } => {
                // Will call NIP-46 RPC get_public_key in a follow-up iteration
                Err(js_err("NIP-46 getPublicKey not implemented yet"))
            }
            ActiveSigner::Unset => Err(js_err("signer not configured")),
        }
    }

    /// Direct path: sign event template (JSON object string). Returns signed event (JSON string).
    #[wasm_bindgen(js_name = "signEventDirect")]
    pub async fn sign_event_direct(&self, template_json: String) -> Result<JsValue, JsValue> {
        let tmpl: serde_json::Value = serde_json::from_str(&template_json)
            .map_err(|e| js_err(&format!("invalid template: {e}")))?;

        match &*self.active.borrow() {
            ActiveSigner::PrivateKey { .. } => {
                // TODO: sign using your pk signer; return full event JSON
                Ok(JsValue::from_str(
                    &serde_json::to_string(&tmpl).unwrap_or_else(|_| "{}".to_string()),
                ))
            }
            ActiveSigner::Nip07 => {
                // window.nostr.signEvent(template)
                let win = web_sys::window().ok_or_else(|| js_err("no window"))?;
                let nostr = js_get(&win, "nostr").ok_or_else(|| js_err("window.nostr missing"))?;
                let sign_fn = js_get_fn(&nostr, "signEvent")
                    .ok_or_else(|| js_err("nostr.signEvent missing"))?;
                let js_evt =
                    serde_wasm_bindgen::to_value(&tmpl).map_err(|e| js_err(&e.to_string()))?;
                let promise = sign_fn
                    .call1(&nostr, &js_evt)
                    .map_err(|_| js_err("nostr.signEvent call failed"))?;
                let signed = wasm_bindgen_futures::JsFuture::from(js_as_promise(promise)?)
                    .await
                    .map_err(|_| js_err("nostr.signEvent rejected"))?;
                Ok(signed)
            }
            ActiveSigner::Nip46 { .. } => {
                // TODO: NIP-46 RPC sign_event
                Err(js_err("NIP-46 signEvent not implemented yet"))
            }
            ActiveSigner::Unset => Err(js_err("signer not configured")),
        }
    }
}

impl Signer {
    // Service loop: drains signer_service_request and writes signer_service_response.
    // For now, this echoes the payload back in a trivial envelope to prove the pipe.
    fn start_service_loop(&self) {
        let svc_req = self.svc_req.clone();
        let svc_resp = self.svc_resp.clone();

        spawn_local(async move {
            let mut sleep_ms: u32 = 8;
            let max_sleep: u32 = 256;

            loop {
                let maybe = { svc_req.borrow_mut().read_next() };

                if let Some(bytes) = maybe {
                    // Placeholder: In the real version, decode FB::SignerRequest, dispatch
                    // to the active signer, then encode FB::SignerResponse.
                    // For now, echo a minimal "ok" envelope: { "ok": true, "echo": ... }
                    sleep_ms = 8;

                    // Try to log a UTF-8 preview
                    if let Ok(s) = std::str::from_utf8(&bytes) {
                        info!(
                            "[signer][svc] received {} bytes: {}",
                            bytes.len(),
                            preview(s, 160)
                        );
                    } else {
                        info!("[signer][svc] received {} binary bytes", bytes.len());
                    }

                    // Echo back as an opaque payload so parser can validate the pipe.
                    if !bytes.is_empty() {
                        svc_resp.borrow_mut().write(&bytes);
                    } else {
                        // Always write something, even if empty, to test ring pathway
                        let fallback = br#"{"ok":true}"#;
                        svc_resp.borrow_mut().write(fallback);
                    }

                    continue;
                }

                TimeoutFuture::new(sleep_ms).await;
                sleep_ms = (sleep_ms * 2).min(max_sleep);
            }
        });

        info!("[signer] service loop spawned");
    }

    // NIP-46 pump: drains ws_response_signer and logs frames.
    // Later this will decrypt and correlate RPC replies by id.
    fn start_nip46_pump(&self) {
        let ws_resp = self.ws_resp.clone();

        spawn_local(async move {
            let mut sleep_ms: u32 = 8;
            let max_sleep: u32 = 256;

            loop {
                let maybe = { ws_resp.borrow_mut().read_next() };

                if let Some(bytes) = maybe {
                    sleep_ms = 8;

                    // Typically this is a fb::WorkerMessage buffer. Until we have the FB
                    // schema/module in place here, we just log size and a short preview.
                    // If upstream writes raw UTF-8 frames, we'll show them.
                    if let Ok(s) = std::str::from_utf8(&bytes) {
                        info!(
                            "[signer][nip46] ws_response {} bytes: {}",
                            bytes.len(),
                            preview(s, 160)
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

        info!("[signer] NIP-46 response pump spawned");
    }

    // Helper to send Envelope { relays, frames } to connections via ws_request_signer.
    // connections expects JSON that matches its Envelope { relays: Vec<String>, frames: Vec<String> }.
    #[allow(unused)]
    fn publish_frames(&self, relays: &[String], frames: &[String]) -> Result<(), JsValue> {
        let env = serde_json::json!({
            "relays": relays,
            "frames": frames,
        });
        let bytes = serde_json::to_vec(&env)
            .map_err(|e| JsValue::from_str(&format!("serialize envelope: {}", e)))?;
        let ok = self.ws_req.borrow_mut().write(&bytes);
        if !ok {
            warn!("[signer] ws_req ring full, frame dropped");
        }
        Ok(())
    }
}

// ----------------------
// Small JS interop utils
// ----------------------

fn js_err(msg: &str) -> JsValue {
    JsValue::from_str(msg)
}

fn js_get(obj: &wasm_bindgen::JsValue, key: &str) -> Option<wasm_bindgen::JsValue> {
    js_sys::Reflect::get(obj, &JsValue::from_str(key)).ok()
}

fn js_get_fn(obj: &wasm_bindgen::JsValue, key: &str) -> Option<js_sys::Function> {
    js_sys::Reflect::get(obj, &JsValue::from_str(key))
        .ok()
        .and_then(|v| v.dyn_into::<js_sys::Function>().ok())
}

fn js_as_promise(v: wasm_bindgen::JsValue) -> Result<js_sys::Promise, JsValue> {
    v.dyn_into::<js_sys::Promise>()
        .map_err(|_| js_err("value is not a Promise"))
}

fn preview(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let head = &s[..max];
        format!("{}…(+{} bytes)", head, s.len() - max)
    }
}

// Re-export SabRing for convenience (optional)
#[allow(unused)]
pub use shared::SabRing;
