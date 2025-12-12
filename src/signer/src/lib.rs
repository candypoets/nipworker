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
mod service;
mod signers;
pub use client::SignerClient;
use signers::{Nip07Signer, Nip46Config, Nip46Signer, PrivateKeySigner};

use crate::signers::SignerError;

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

enum ActiveSigner {
    Unset,
    Pk(PrivateKeySigner),
    Nip07(Nip07Signer),
    Nip46(Nip46Signer),
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
        info!("[signer] loops started");
    }

    // --------------------------
    // Direct methods (bypass SAB)
    // --------------------------

    /// Set a private key signer (hex or nsec). For now we don't perform validation here.
    #[wasm_bindgen(js_name = "setPrivateKey")]
    pub fn set_private_key(&self, secret: String) -> Result<(), JsValue> {
        let pk = PrivateKeySigner::new(&secret)
            .map_err(|e| js_err(&format!("failed to create private key signer: {e}")))?;
        *self.active.borrow_mut() = ActiveSigner::Pk(pk);
        info!("[signer] active signer = PrivateKey");
        Ok(())
    }

    /// Use NIP-07 (window.nostr). Assumes this signer runs in the main window context.
    #[wasm_bindgen(js_name = "setNip07")]
    pub fn set_nip07(&self) {
        *self.active.borrow_mut() = ActiveSigner::Nip07(Nip07Signer::new());
        info!("[signer] active signer = NIP-07");
    }

    /// Configure NIP-46 remote signer. Takes remote signer pubkey (hex) and relays.
    #[wasm_bindgen(js_name = "setNip46")]
    pub fn set_nip46(&self, remote_pubkey_hex: String, relays: Array) -> Result<(), JsValue> {
        let relays_vec: Vec<String> = relays.iter().filter_map(|v| v.as_string()).collect();
        let cfg = Nip46Config {
            remote_signer_pubkey: remote_pubkey_hex,
            relays: relays_vec,
            use_nip44: true,
            app_name: None,
        };
        // Create fresh SAB views for the NIP-46 signer
        let nip46 = Nip46Signer::new(cfg, self.ws_req.clone(), self.ws_resp.clone());
        nip46.start();
        *self.active.borrow_mut() = ActiveSigner::Nip46(nip46);
        info!("[signer] active signer = NIP-46 (configured)");
        Ok(())
    }

    /// Direct path: get public key as a string. This is a placeholder until key plumbing lands.
    #[wasm_bindgen(js_name = "getPublicKeyDirect")]
    pub async fn get_public_key_direct(&self) -> Result<JsValue, JsValue> {
        match &*self.active.borrow() {
            ActiveSigner::Pk(pk) => {
                let pk_hex = pk.get_public_key().map_err(|e| js_err(&e.to_string()))?;
                Ok(JsValue::from_str(&pk_hex))
            }
            ActiveSigner::Nip07(s) => {
                let pk = s.get_public_key().await?;

                Ok(JsValue::from_str(&pk))
            }
            ActiveSigner::Nip46(s) => {
                let pk = s.get_public_key().await?;
                Ok(JsValue::from_str(&pk))
            }
            ActiveSigner::Unset => Err(js_err("signer not configured")),
        }
    }

    /// Direct path: sign event template (JSON object string). Returns signed event (JSON string).
    #[wasm_bindgen(js_name = "signEvent")]
    pub async fn sign_event_direct(&self, tmpl: String) -> Result<JsValue, JsValue> {
        match &*self.active.borrow() {
            ActiveSigner::Pk(pk) => {
                let signed = pk
                    .sign_event(&tmpl)
                    .await
                    .map_err(|e| js_err(&e.to_string()))?;
                Ok(JsValue::from_str(
                    &serde_json::to_string(&signed).unwrap_or_else(|_| "{}".to_string()),
                ))
            }
            ActiveSigner::Nip07(s) => {
                let signed = s.sign_event(&tmpl).await?;
                Ok(JsValue::from_str(
                    &serde_json::to_string(&signed).unwrap_or_else(|_| "{}".to_string()),
                ))
            }
            ActiveSigner::Nip46(s) => {
                let signed = s.sign_event(&tmpl).await?;
                Ok(JsValue::from_str(
                    &serde_json::to_string(&signed).unwrap_or_else(|_| "{}".to_string()),
                ))
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
