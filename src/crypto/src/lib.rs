#![allow(clippy::needless_return)]
#![allow(clippy::should_implement_trait)]
#![allow(dead_code)]

use js_sys::SharedArrayBuffer;
use shared::{init_with_component, types::Keys, utils::crypto::verify_proof_dleq_string, SabRing};
use tracing::{error, info, warn};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

use gloo_timers::future::TimeoutFuture;
use std::cell::RefCell;
use std::rc::Rc;
use url::Url;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_name = postMessage)]
    fn post_message(msg: &JsValue);
}

mod crypto_utils;
mod signers;
pub use crypto_utils::hash_to_curve;
use signers::{Nip07Signer, Nip46Config, Nip46Signer, PrivateKeySigner};

/// Helper structs for URL parsing
#[derive(Debug)]
struct BunkerUrl {
    remote_pubkey: String,
    relays: Vec<String>,
    secret: Option<String>,
}

#[derive(Debug)]
struct NostrconnectUrl {
    client_pubkey: String,
    relays: Vec<String>,
    secret: String,
    app_name: Option<String>,
}

/// A cryptographic operations worker that:
/// - Wires four SAB rings:
///   - crypto_service_request  (parser -> crypto)
///   - crypto_service_response (crypto -> parser)
///   - ws_request_crypto       (crypto -> connections)
///   - ws_response_crypto      (connections -> crypto)
/// - Exposes direct methods for frontend (bypassing SAB).
///
/// NOTE:
/// - The service protocol (SignerRequest/SignerResponse) is intended to be FlatBuffers.
///   This initial version simply echoes back payloads to prove the pipe and will
///   be swapped to FlatBuffers once the schema is generated in `generated`.
/// - NIP-46 transport wiring is scaffolded. It publishes/consumes frames via the
///   dedicated ws SABs. Business logic will be implemented after schema/types land.
#[wasm_bindgen]
pub struct Crypto {
    // Parser <-> Crypto SABs (SPSC each)
    svc_req: Rc<RefCell<SabRing>>,
    svc_resp: Rc<RefCell<SabRing>>,

    // Crypto <-> Connections SABs (SPSC each)
    ws_req: Rc<RefCell<SabRing>>,
    ws_resp: Rc<RefCell<SabRing>>,

    // Simple runtime state
    active: Rc<RefCell<ActiveSigner>>,
}

#[derive(Clone)]
enum ActiveSigner {
    Unset,
    Pk(Rc<PrivateKeySigner>),
    Nip07(Rc<Nip07Signer>),
    Nip46(Rc<Nip46Signer>),
}

#[wasm_bindgen]
impl Crypto {
    /// new(crypto_service_request, crypto_service_response, ws_request_crypto, ws_response_crypto)
    #[wasm_bindgen(constructor)]
    pub fn new(
        crypto_service_request: SharedArrayBuffer,
        crypto_service_response: SharedArrayBuffer,
        ws_request_crypto: SharedArrayBuffer,
        ws_response_crypto: SharedArrayBuffer,
    ) -> Result<Crypto, JsValue> {
        init_with_component(tracing::Level::ERROR, "crypto");

        let svc_req = Rc::new(RefCell::new(SabRing::new(crypto_service_request)?));
        let svc_resp = Rc::new(RefCell::new(SabRing::new(crypto_service_response)?));
        let ws_req = Rc::new(RefCell::new(SabRing::new(ws_request_crypto)?));
        let ws_resp = Rc::new(RefCell::new(SabRing::new(ws_response_crypto)?));

        info!("[crypto] initialized SAB rings");

        let crypto = Crypto {
            svc_req,
            svc_resp,
            ws_req,
            ws_resp,
            active: Rc::new(RefCell::new(ActiveSigner::Unset)),
        };

        crypto.start_service_loop();

        Ok(crypto)
    }

    // --------------------------
    // Direct methods (bypass SAB)
    // --------------------------

    /// Set a private key signer (hex or nsec). For now we don't perform validation here.
    #[wasm_bindgen(js_name = "setPrivateKey")]
    pub fn set_private_key(&self, secret: String) -> Result<(), JsValue> {
        info!("[crypto] setting private key");
        let pk = PrivateKeySigner::new(&secret)
            .map_err(|e| js_err(&format!("failed to create private key signer: {e}")))?;
        *self.active.borrow_mut() = ActiveSigner::Pk(Rc::new(pk));
        info!("[crypto] active signer = PrivateKey");
        Ok(())
    }

    /// Use NIP-07 (window.nostr). Assumes this signer runs in the main window context.
    #[wasm_bindgen(js_name = "setNip07")]
    pub fn set_nip07(&self) {
        *self.active.borrow_mut() = ActiveSigner::Nip07(Rc::new(Nip07Signer::new()));
        info!("[crypto] active signer = NIP-07");
    }

    /// Configure NIP-46 remote signer. Takes remote signer pubkey (hex) and relays.
    /// Configure NIP-46 remote signer using a bunker URL.
    /// This is for the "Direct connection initiated by remote-signer" flow.
    #[wasm_bindgen(js_name = "setNip46Bunker")]
    pub fn set_nip46_bunker(
        &self,
        bunker_url: String,
        client_secret: Option<String>,
    ) -> Result<(), JsValue> {
        // Parse the bunker URL to extract remote pubkey and relays
        let parsed = Self::parse_bunker_url(&bunker_url)?;

        info!(
            "[crypto] setting NIP-46 signer with remote_pubkey: {}, relays: {:?}",
            parsed.remote_pubkey, parsed.relays
        );

        let cfg = Nip46Config {
            remote_signer_pubkey: parsed.remote_pubkey,
            relays: parsed.relays,
            use_nip44: true,
            app_name: None,
            expected_secret: parsed.secret, // Optional secret for single-use connection
        };

        let client_keys = if let Some(s) = client_secret {
            Some(Keys::parse(&s).map_err(|e| js_err(&e.to_string()))?)
        } else {
            None
        };

        // Create fresh SAB views for the NIP-46 signer
        let nip46 = Rc::new(Nip46Signer::new(
            cfg,
            self.ws_req.clone(),
            self.ws_resp.clone(),
            client_keys,
        ));
        let signer_weak = Rc::downgrade(&self.active);
        nip46.start(Some(Rc::new(move |_| {
            if let Some(active_rc) = signer_weak.upgrade() {
                if let ActiveSigner::Nip46(ref n46) = *active_rc.borrow() {
                    if let Some(url) = n46.get_bunker_url() {
                        // We need a way to call notify_discovery from here.
                        // Since we don't have &self, we can't call it directly.
                        // But we can use the global post_message.
                        let msg = js_sys::Object::new();
                        let _ =
                            js_sys::Reflect::set(&msg, &"type".into(), &"bunker_discovered".into());
                        let _ = js_sys::Reflect::set(&msg, &"bunker_url".into(), &url.into());
                        post_message(&msg.into());
                    }
                }
            }
        })));
        *self.active.borrow_mut() = ActiveSigner::Nip46(nip46.clone());

        // Auto-connect for bunker flow
        spawn_local(async move {
            info!("[nip46] Auto-connecting to bunker...");
            if let Err(e) = nip46.connect().await {
                error!("[nip46] Auto-connect failed: {:?}", e);
            } else {
                info!("[nip46] Auto-connect successful");
            }
        });

        info!("[crypto] active signer = NIP-46 (Bunker mode)");
        Ok(())
    }

    /// Configure NIP-46 remote signer using a QR code.
    /// This is for the "Direct connection initiated by the client" flow.
    #[wasm_bindgen(js_name = "setNip46QR")]
    pub fn set_nip46_qr(
        &self,
        nostrconnect_url: String,
        client_secret: Option<String>,
    ) -> Result<(), JsValue> {
        // Parse the nostrconnect URL
        let parsed = Self::parse_nostrconnect_url(&nostrconnect_url)?;

        info!(
            "[crypto] setting NIP-46 signer via QR code (client_pubkey: {})",
            parsed.client_pubkey
        );

        let cfg = Nip46Config {
            remote_signer_pubkey: String::new(), // Will be discovered
            relays: parsed.relays,
            use_nip44: true,
            app_name: parsed.app_name,
            expected_secret: Some(parsed.secret), // Required for validation
        };

        let client_keys = if let Some(s) = client_secret {
            Some(Keys::parse(&s).map_err(|e| js_err(&e.to_string()))?)
        } else {
            None
        };

        // Create fresh SAB views for the NIP-46 signer
        let nip46 = Rc::new(Nip46Signer::new(
            cfg,
            self.ws_req.clone(),
            self.ws_resp.clone(),
            client_keys,
        ));
        let signer_weak = Rc::downgrade(&self.active);
        nip46.start(Some(Rc::new(move |_| {
            if let Some(active_rc) = signer_weak.upgrade() {
                if let ActiveSigner::Nip46(ref n46) = *active_rc.borrow() {
                    if let Some(url) = n46.get_bunker_url() {
                        let msg = js_sys::Object::new();
                        let _ =
                            js_sys::Reflect::set(&msg, &"type".into(), &"bunker_discovered".into());
                        let _ = js_sys::Reflect::set(&msg, &"bunker_url".into(), &url.into());
                        post_message(&msg.into());
                    }
                }
            }
        })));
        *self.active.borrow_mut() = ActiveSigner::Nip46(nip46.clone());

        // Auto-connect for bunker flow
        spawn_local(async move {
            info!("[nip46] Auto-connecting to bunker...");
            if let Err(e) = nip46.connect().await {
                error!("[nip46] Auto-connect failed: {:?}", e);
            } else {
                info!("[nip46] Auto-connect successful");
            }
        });

        info!("[crypto] active signer = NIP-46 (QR code discovery mode)");
        Ok(())
    }

    /// Helper function to parse bunker URLs
    fn parse_bunker_url(url: &str) -> Result<BunkerUrl, JsValue> {
        if !url.starts_with("bunker://") {
            return Err(JsValue::from_str(
                "Invalid bunker URL: must start with bunker://",
            ));
        }

        let url_part = &url[9..]; // Remove 'bunker://'
        let parts: Vec<&str> = url_part.split('?').collect();

        if parts.len() != 2 {
            return Err(JsValue::from_str(
                "Invalid bunker URL: missing query parameters",
            ));
        }

        let remote_pubkey = parts[0];
        if !remote_pubkey.chars().all(|c| c.is_ascii_hexdigit()) || remote_pubkey.len() != 64 {
            return Err(JsValue::from_str(
                "Invalid remote signer pubkey in bunker URL",
            ));
        }

        let params = Url::parse(&format!("http://localhost/?{}", parts[1]))
            .map_err(|e| JsValue::from_str(&format!("Invalid URL parameters: {}", e)))?;

        let mut relays = Vec::new();
        for relay in params
            .query_pairs()
            .filter_map(|(k, v)| if k == "relay" { Some(v) } else { None })
        {
            relays.push(relay.to_string());
        }

        if relays.is_empty() {
            return Err(JsValue::from_str("No relays specified in bunker URL"));
        }

        let secret = params.query_pairs().find_map(|(k, v)| {
            if k == "secret" {
                Some(v.to_string())
            } else {
                None
            }
        });

        Ok(BunkerUrl {
            remote_pubkey: remote_pubkey.to_string(),
            relays,
            secret,
        })
    }

    /// Helper function to parse nostrconnect URLs
    fn parse_nostrconnect_url(url: &str) -> Result<NostrconnectUrl, JsValue> {
        info!("Parsing nostrconnect URL: {}", url);
        if !url.starts_with("nostrconnect://") {
            return Err(JsValue::from_str(
                "Invalid nostrconnect URL: must start with nostrconnect://",
            ));
        }

        let url_part = &url[15..]; // Remove 'nostrconnect://'
        let parts: Vec<&str> = url_part.split('?').collect();

        if parts.len() != 2 {
            return Err(JsValue::from_str(
                "Invalid nostrconnect URL: missing query parameters",
            ));
        }

        let client_pubkey = parts[0];
        if !client_pubkey.chars().all(|c| c.is_ascii_hexdigit()) || client_pubkey.len() != 64 {
            return Err(JsValue::from_str(
                "Invalid client pubkey in nostrconnect URL",
            ));
        }

        let params = Url::parse(&format!("http://localhost/?{}", parts[1]))
            .map_err(|e| JsValue::from_str(&format!("Invalid URL parameters: {}", e)))?;

        let mut relays = Vec::new();
        for relay in params
            .query_pairs()
            .filter_map(|(k, v)| if k == "relay" { Some(v) } else { None })
        {
            relays.push(relay.to_string());
        }

        if relays.is_empty() {
            return Err(JsValue::from_str("No relays specified in nostrconnect URL"));
        }

        let secret = params
            .query_pairs()
            .find_map(|(k, v)| {
                if k == "secret" {
                    Some(v.to_string())
                } else {
                    None
                }
            })
            .ok_or_else(|| JsValue::from_str("Secret is required in nostrconnect URL"))?;

        let app_name = params.query_pairs().find_map(|(k, v)| {
            if k == "name" {
                Some(v.to_string())
            } else {
                None
            }
        });

        Ok(NostrconnectUrl {
            client_pubkey: client_pubkey.to_string(),
            relays,
            secret,
            app_name,
        })
    }

    /// Get the discovered remote signer pubkey (for QR code mode).
    /// Returns None if no remote signer has been discovered yet.
    #[wasm_bindgen(js_name = "getDiscoveredRemotePubkey")]
    pub fn get_discovered_remote_pubkey(&self) -> Option<String> {
        if let ActiveSigner::Nip46(ref nip46) = *self.active.borrow() {
            nip46.get_discovered_remote_pubkey()
        } else {
            None
        }
    }

    /// Notify the main thread about a discovered bunker URL.
    pub fn notify_discovery(&self, bunker_url: &str) {
        let msg = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&msg, &"type".into(), &"bunker_discovered".into());
        let _ = js_sys::Reflect::set(&msg, &"bunker_url".into(), &bunker_url.into());
        post_message(&msg.into());
    }

    /// Direct path: perform NIP-46 connect handshake.
    #[wasm_bindgen(js_name = "connectDirect")]
    pub async fn connect_direct(&self) -> Result<JsValue, JsValue> {
        let signer = match &*self.active.borrow() {
            ActiveSigner::Nip46(s) => s.clone(),
            _ => return Err(js_err("connect only supported for NIP-46")),
        };
        let res = signer.connect().await?;
        Ok(JsValue::from_str(&res))
    }

    /// Direct path: get public key as a string. This is a placeholder until key plumbing lands.
    #[wasm_bindgen(js_name = "getPublicKeyDirect")]
    pub async fn get_public_key_direct(&self) -> Result<JsValue, JsValue> {
        let active = self.active.borrow().clone();
        match active {
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
            ActiveSigner::Unset => Err(js_err("crypto not configured")),
        }
    }

    /// Direct path: sign event template (JSON object string). Returns signed event (JSON string).
    #[wasm_bindgen(js_name = "signEvent")]
    pub async fn sign_event_direct(&self, tmpl: String) -> Result<JsValue, JsValue> {
        let active = self.active.borrow().clone();
        match active {
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
            ActiveSigner::Unset => Err(js_err("crypto not configured")),
        }
    }
}

impl Crypto {
    // Service loop: drains signer_service_request and writes signer_service_response.
    // For now, this echoes the payload back in a trivial envelope to prove the pipe.
    fn start_service_loop(&self) {
        let svc_req = self.svc_req.clone();
        let svc_resp = self.svc_resp.clone();
        let active = self.active.clone();

        spawn_local(async move {
            let mut sleep_ms: u32 = 8;
            let max_sleep: u32 = 256;

            loop {
                let maybe = { svc_req.borrow_mut().read_next() };

                if let Some(bytes) = maybe {
                    sleep_ms = 8;

                    // Decode SignerRequest FlatBuffer
                    let req = match flatbuffers::root::<shared::generated::nostr::fb::SignerRequest>(
                        &bytes,
                    ) {
                        Ok(r) => r,
                        Err(e) => {
                            warn!("[crypto][svc] failed to decode SignerRequest: {:?}", e);
                            continue;
                        }
                    };

                    let rid = req.request_id();
                    let op = req.op();
                    let payload = req.payload().unwrap_or("");
                    let peer = req.pubkey().unwrap_or("");
                    let sender = req.sender_pubkey().unwrap_or("");
                    let recipient = req.recipient_pubkey().unwrap_or("");

                    // Handle operations that don't require a signer
                    let result: Result<String, String> = if op
                        == shared::generated::nostr::fb::SignerOp::VerifyProof
                    {
                        verify_proof_and_return_y_point(payload)
                    } else {
                        // Dispatch to active signer for signer-dependent operations
                        match &*active.borrow() {
                            ActiveSigner::Unset => Err("crypto not configured".into()),
                            ActiveSigner::Pk(pk) => match op {
                                shared::generated::nostr::fb::SignerOp::GetPubkey => {
                                    pk.get_public_key().map_err(|e| e.to_string())
                                }
                                shared::generated::nostr::fb::SignerOp::SignEvent => {
                                    match pk.sign_event(payload).await {
                                        Ok(sig) => Ok(sig),
                                        Err(e) => Err(e.to_string()),
                                    }
                                }
                                shared::generated::nostr::fb::SignerOp::Nip04Encrypt => {
                                    pk.nip04_encrypt(peer, payload).map_err(|e| e.to_string())
                                }
                                shared::generated::nostr::fb::SignerOp::Nip04Decrypt => {
                                    pk.nip04_decrypt(peer, payload).map_err(|e| e.to_string())
                                }
                                shared::generated::nostr::fb::SignerOp::Nip44Encrypt => {
                                    pk.nip44_encrypt(peer, payload).map_err(|e| e.to_string())
                                }
                                shared::generated::nostr::fb::SignerOp::Nip44Decrypt => {
                                    pk.nip44_decrypt(peer, payload).map_err(|e| e.to_string())
                                }
                                shared::generated::nostr::fb::SignerOp::Nip04DecryptBetween => {
                                    info!("[crypto] decrypting between");
                                    if sender.is_empty() && recipient.is_empty() {
                                        Err("missing sender/recipient".to_string())
                                    } else {
                                        pk.nip04_decrypt_between(sender, recipient, payload)
                                            .map_err(|e| e.to_string())
                                    }
                                }
                                shared::generated::nostr::fb::SignerOp::Nip44DecryptBetween => {
                                    if sender.is_empty() && recipient.is_empty() {
                                        Err("missing sender/recipient".to_string())
                                    } else {
                                        pk.nip44_decrypt_between(sender, recipient, payload)
                                            .map_err(|e| e.to_string())
                                    }
                                }
                                _ => Err("Unsupported operation".to_string()),
                            },
                            ActiveSigner::Nip07(s) => match op {
                                shared::generated::nostr::fb::SignerOp::GetPubkey => {
                                    s.get_public_key().await.map_err(|e| format!("{:?}", e))
                                }
                                shared::generated::nostr::fb::SignerOp::SignEvent => {
                                    match s.sign_event(payload).await {
                                        Ok(signed) => Ok(serde_json::to_string(&signed)
                                            .unwrap_or_else(|_| "{}".to_string())),
                                        Err(e) => Err(format!("{:?}", e)),
                                    }
                                }
                                shared::generated::nostr::fb::SignerOp::Nip04Encrypt => s
                                    .nip04_encrypt(peer, payload)
                                    .await
                                    .map_err(|e| format!("{:?}", e)),
                                shared::generated::nostr::fb::SignerOp::Nip04Decrypt => s
                                    .nip04_decrypt(peer, payload)
                                    .await
                                    .map_err(|e| format!("{:?}", e)),
                                shared::generated::nostr::fb::SignerOp::Nip44Encrypt => s
                                    .nip44_encrypt(peer, payload)
                                    .await
                                    .map_err(|e| format!("{:?}", e)),
                                shared::generated::nostr::fb::SignerOp::Nip44Decrypt => s
                                    .nip44_decrypt(peer, payload)
                                    .await
                                    .map_err(|e| format!("{:?}", e)),
                                shared::generated::nostr::fb::SignerOp::Nip04DecryptBetween => {
                                    if sender.is_empty() && recipient.is_empty() {
                                        Err("missing sender/recipient".to_string())
                                    } else {
                                        s.nip04_decrypt_between(sender, recipient, payload)
                                            .await
                                            .map_err(|e| format!("{:?}", e))
                                    }
                                }
                                shared::generated::nostr::fb::SignerOp::Nip44DecryptBetween => {
                                    if sender.is_empty() && recipient.is_empty() {
                                        Err("missing sender/recipient".to_string())
                                    } else {
                                        s.nip44_decrypt_between(sender, recipient, payload)
                                            .await
                                            .map_err(|e| format!("{:?}", e))
                                    }
                                }
                                _ => Err("Unsupported operation".to_string()),
                            },
                            ActiveSigner::Nip46(s) => match op {
                                shared::generated::nostr::fb::SignerOp::GetPubkey => {
                                    s.get_public_key().await.map_err(|e| format!("{:?}", e))
                                }
                                shared::generated::nostr::fb::SignerOp::SignEvent => {
                                    match s.sign_event(payload).await {
                                        Ok(signed) => Ok(serde_json::to_string(&signed)
                                            .unwrap_or_else(|_| "{}".to_string())),
                                        Err(e) => Err(format!("{:?}", e)),
                                    }
                                }
                                shared::generated::nostr::fb::SignerOp::Nip04Encrypt => {
                                    match s.nip04_encrypt(peer, payload).await {
                                        Ok(out) => Ok(out),
                                        Err(e) => Err(format!("{:?}", e)),
                                    }
                                }
                                shared::generated::nostr::fb::SignerOp::Nip04Decrypt => {
                                    match s.nip04_decrypt(peer, payload).await {
                                        Ok(out) => Ok(out),
                                        Err(e) => Err(format!("{:?}", e)),
                                    }
                                }
                                shared::generated::nostr::fb::SignerOp::Nip44Encrypt => {
                                    match s.nip44_encrypt(peer, payload).await {
                                        Ok(out) => Ok(out),
                                        Err(e) => Err(format!("{:?}", e)),
                                    }
                                }
                                shared::generated::nostr::fb::SignerOp::Nip44Decrypt => {
                                    match s.nip44_decrypt(peer, payload).await {
                                        Ok(out) => Ok(out),
                                        Err(e) => Err(format!("{:?}", e)),
                                    }
                                }
                                shared::generated::nostr::fb::SignerOp::Nip04DecryptBetween => {
                                    match s.nip04_decrypt_between(sender, recipient, payload).await
                                    {
                                        Ok(out) => Ok(out),
                                        Err(e) => Err(format!("{:?}", e)),
                                    }
                                }
                                shared::generated::nostr::fb::SignerOp::Nip44DecryptBetween => {
                                    match s.nip44_decrypt_between(sender, recipient, payload).await
                                    {
                                        Ok(out) => Ok(out),
                                        Err(e) => Err(format!("{:?}", e)),
                                    }
                                }
                                _ => Err("Unsupported operation".to_string()),
                            },
                        }
                    };

                    // Encode SignerResponse FlatBuffer (no ok flag; use result/error)
                    let mut fbb = flatbuffers::FlatBufferBuilder::new();
                    let (result_off, err_off) = match result {
                        Ok(s) => (Some(fbb.create_string(&s)), None),
                        Err(e) => (None, Some(fbb.create_string(&e))),
                    };
                    let resp = shared::generated::nostr::fb::SignerResponse::create(
                        &mut fbb,
                        &shared::generated::nostr::fb::SignerResponseArgs {
                            request_id: rid,
                            result: result_off,
                            error: err_off,
                        },
                    );
                    fbb.finish(resp, None);
                    let out = fbb.finished_data();
                    let _ = svc_resp.borrow_mut().write(out);

                    continue;
                }

                TimeoutFuture::new(sleep_ms).await;
                sleep_ms = (sleep_ms * 2).min(max_sleep);
            }
        });

        info!("[crypto] service loop spawned");
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
            warn!("[crypto] ws_req ring full, frame dropped");
        }
        Ok(())
    }
}

/// Verify proof DLEQ and return Y point if valid
fn verify_proof_and_return_y_point(payload: &str) -> Result<String, String> {
    // Parse JSON payload to extract proof and mint_keys
    let (proof, keys_map) =
        shared::utils::crypto::parse_verification_payload(payload).map_err(|e| e.to_string())?;

    // Verify DLEQ with mint keys
    if shared::utils::crypto::verify_proof_dleq_with_keys(&proof, &keys_map) {
        // DLEQ valid - compute and return Y point
        let y_point = shared::crypto::compute_y_point(&proof.secret);
        info!("dleq valid");
        Ok(y_point)
    } else {
        info!("dleq invalid");
        // DLEQ invalid - return empty string
        Ok(String::new())
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
