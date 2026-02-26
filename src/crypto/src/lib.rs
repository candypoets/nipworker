#![allow(clippy::needless_return)]
#![allow(clippy::should_implement_trait)]
#![allow(dead_code)]

use shared::{init_with_component, types::Keys, utils::crypto::verify_proof_dleq_string, Port};
use tracing::{error, info, warn};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::MessagePort;

use futures_channel::mpsc;
use futures_util::select;
use futures_util::FutureExt;
use futures_util::StreamExt;
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
/// - Wires four MessageChannel ports:
///   - from_parser  (parser -> crypto): SignerRequest messages
///   - to_parser    (crypto -> parser): SignerResponse messages
///   - to_connections (crypto -> connections): NIP-46 frames
///   - from_connections (connections -> crypto): NIP-46 responses
///   - to_main      (crypto -> main): Control responses
/// - Exposes direct methods for frontend (bypass MessageChannel).
///
/// NOTE:
/// - The service protocol (SignerRequest/SignerResponse) uses FlatBuffers.
/// - NIP-46 transport uses MessageChannel ports instead of SAB rings.
#[wasm_bindgen]
pub struct Crypto {
    // Parser <-> Crypto ports
    to_parser: Rc<RefCell<Port>>,

    // Crypto <-> Connections ports for NIP-46
    to_connections: Rc<RefCell<Port>>,
    from_connections: MessagePort, // Stored for NIP-46 signer initialization

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
    /// new(toMain, fromParser, toConnections, fromConnections, toParser)
    ///
    /// Parameters:
    /// - to_main: MessagePort for sending control responses to main thread
    /// - from_parser: MessagePort for receiving SignerRequest from parser
    /// - to_connections: MessagePort for sending NIP-46 frames to connections
    /// - from_connections: MessagePort for receiving NIP-46 responses from connections
    /// - to_parser: MessagePort for sending SignerResponse to parser
    #[wasm_bindgen(constructor)]
    pub fn new(
        to_main: MessagePort,
        from_parser: MessagePort,
        to_connections: MessagePort,
        from_connections: MessagePort,
        to_parser: MessagePort,
    ) -> Result<Crypto, JsValue> {
        init_with_component(tracing::Level::INFO, "crypto");

        // Create receivers from MessagePorts
        let from_parser_rx = Port::from_receiver(from_parser);
        // Clone from_connections before moving it into Port::from_receiver
        let from_connections_clone = from_connections.clone();
        let from_connections_rx = Port::from_receiver(from_connections);

        // Wrap sender ports
        let to_parser_port = Rc::new(RefCell::new(Port::new(to_parser)));
        let to_connections_port = Rc::new(RefCell::new(Port::new(to_connections)));
        let to_main_port = Port::new(to_main);

        info!("[crypto] initialized MessageChannel ports");

        let crypto = Crypto {
            to_parser: to_parser_port.clone(),
            to_connections: to_connections_port.clone(),
            from_connections: from_connections_clone,
            active: Rc::new(RefCell::new(ActiveSigner::Unset)),
        };

        crypto.start_service_loop(from_parser_rx, from_connections_rx, to_main_port);

        Ok(crypto)
    }

    // --------------------------
    // Direct methods (bypass SAB)
    // --------------------------

    /// Helper to send signer_ready event to main thread (static for use in closures)
    /// Includes all info needed to reconstruct the session for future reloads
    fn send_signer_ready(signer_type: &str, pubkey: &str, bunker_url: Option<&str>) {
        info!("[crypto] Signer ready: {} - {}", signer_type, &pubkey[..16.min(pubkey.len())]);
        let msg = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&msg, &"type".into(), &"signer_ready".into());
        let _ = js_sys::Reflect::set(&msg, &"signer_type".into(), &signer_type.into());
        let _ = js_sys::Reflect::set(&msg, &"pubkey".into(), &pubkey.into());
        if let Some(url) = bunker_url {
            let _ = js_sys::Reflect::set(&msg, &"bunker_url".into(), &url.into());
        }
        post_message(&msg.into());
    }

    /// Set a private key signer (hex or nsec). For now we don't perform validation here.
    #[wasm_bindgen(js_name = "setPrivateKey")]
    pub fn set_private_key(&self, secret: String) -> Result<(), JsValue> {
        info!("[crypto] setting private key");
        let pk = PrivateKeySigner::new(&secret)
            .map_err(|e| js_err(&format!("failed to create private key signer: {e}")))?;
        let pk_hex = pk.get_public_key().map_err(|e| js_err(&e.to_string()))?;
        *self.active.borrow_mut() = ActiveSigner::Pk(Rc::new(pk));
        info!("[crypto] active signer = PrivateKey, auto-sending pubkey");
        Self::send_signer_ready("privkey", &pk_hex, None);
        Ok(())
    }

    /// Use NIP-07 (window.nostr). Assumes this signer runs in the main window context.
    #[wasm_bindgen(js_name = "setNip07")]
    pub fn set_nip07(&self) {
        *self.active.borrow_mut() = ActiveSigner::Nip07(Rc::new(Nip07Signer::new()));
        info!("[crypto] active signer = NIP-07, fetching pubkey...");
        // NIP-07 needs async fetch - get a copy of the active signer first
        let signer = match &*self.active.borrow() {
            ActiveSigner::Nip07(s) => s.clone(),
            _ => return,
        };
        spawn_local(async move {
            if let Ok(pk) = signer.get_public_key().await {
                Self::send_signer_ready("nip07", &pk, None);
            }
        });
    }

    /// Clear the active signer (logout).
    #[wasm_bindgen(js_name = "clearSigner")]
    pub fn clear_signer(&self) {
        *self.active.borrow_mut() = ActiveSigner::Unset;
        info!("[crypto] signer cleared");
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
        // Clone the stored from_connections port for the NIP-46 signer
        let from_connections = self.from_connections.clone();
        info!("[nip46] Bunker mode: connecting to {}", &bunker_url[..50.min(bunker_url.len())]);

        // Parse the bunker URL to extract remote pubkey and relays
        let parsed = Self::parse_bunker_url(&bunker_url)?;

        let cfg = Nip46Config {
            remote_signer_pubkey: parsed.remote_pubkey,
            relays: parsed.relays,
            use_nip44: true,
            app_name: None,
            expected_secret: parsed.secret, // Optional secret for single-use connection
        };

        let client_keys = client_secret.map(|s| {
            Keys::parse(&s).map_err(|e| js_err(&e.to_string()))
        }).transpose()?;

        let from_connections_rx = Port::from_receiver(from_connections);
        let nip46 = Rc::new(Nip46Signer::new(
            cfg,
            self.to_connections.clone(),
            from_connections_rx,
            client_keys,
        ));
        
        let signer_weak = Rc::downgrade(&self.active);
        nip46.start(Some(Rc::new(move |_| {
            if let Some(active_rc) = signer_weak.upgrade() {
                if let ActiveSigner::Nip46(ref n46) = *active_rc.borrow() {
                    if let Some(url) = n46.get_bunker_url() {
                        info!("[nip46] QR -> Bunker: {}", &url[..60.min(url.len())]);
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

        // Store bunker URL for signer_ready event
        let bunker_url_for_ready = bunker_url.clone();
        
        // Auto-connect for bunker flow and fetch pubkey
        spawn_local(async move {
            match nip46.connect().await {
                Ok(_) => {
                    match nip46.get_public_key().await {
                        Ok(pk) => {
                            Self::send_signer_ready("nip46", &pk, Some(&bunker_url_for_ready));
                        }
                        Err(e) => error!("[nip46] Failed to fetch pubkey: {:?}", e),
                    }
                }
                Err(e) => error!("[nip46] Bunker connect failed: {:?}", e),
            }
        });
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
        // Auto-detect if a bunker URL was passed instead of nostrconnect
        if nostrconnect_url.starts_with("bunker://") {
            info!("[nip46] Auto-detected bunker URL, switching to bunker mode");
            return self.set_nip46_bunker(nostrconnect_url, client_secret);
        }

        // Clone the stored from_connections port for the NIP-46 signer
        let from_connections = self.from_connections.clone();
        info!("[nip46] QR mode: waiting for signer...");

        // Parse the nostrconnect URL
        let parsed = Self::parse_nostrconnect_url(&nostrconnect_url)?;

        let cfg = Nip46Config {
            remote_signer_pubkey: String::new(), // Will be discovered
            relays: parsed.relays,
            use_nip44: true,
            app_name: parsed.app_name,
            expected_secret: Some(parsed.secret), // Required for validation
        };

        let client_keys = client_secret.map(|s| {
            Keys::parse(&s).map_err(|e| js_err(&e.to_string()))
        }).transpose()?;

        let from_connections_rx = Port::from_receiver(from_connections);
        let nip46 = Rc::new(Nip46Signer::new(
            cfg,
            self.to_connections.clone(),
            from_connections_rx,
            client_keys,
        ));

        let signer_weak = Rc::downgrade(&self.active);
        let nip46_for_callback = nip46.clone();
        nip46.start(Some(Rc::new(move |discovered_pk: String| {
            info!("[nip46] Signer discovered: {}...", &discovered_pk[..16.min(discovered_pk.len())]);
            if let Some(active_rc) = signer_weak.upgrade() {
                if let ActiveSigner::Nip46(ref n46) = *active_rc.borrow() {
                    if let Some(bunker_url) = n46.get_bunker_url() {
                        // Update crypto with discovered remote pubkey and fetch user pubkey
                        nip46_for_callback.update_crypto_remote_pubkey(&discovered_pk);
                        let n46_clone = nip46_for_callback.clone();
                        spawn_local(async move {
                            match n46_clone.get_public_key().await {
                                Ok(pk) => {
                                    Self::send_signer_ready("nip46", &pk, Some(&bunker_url));
                                }
                                Err(e) => error!("[nip46] Failed to fetch pubkey: {:?}", e),
                            }
                        });
                    }
                }
            }
        })));
        *self.active.borrow_mut() = ActiveSigner::Nip46(nip46.clone());
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

    /// Auto-fetch and send pubkey to main thread after successful connection
    fn send_pubkey_response(&self, pk: String) {
        info!("[crypto] Auto-sending pubkey to main thread: {}", &pk[..16.min(pk.len())]);
        let msg = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&msg, &"type".into(), &"response".into());
        let _ = js_sys::Reflect::set(&msg, &"op".into(), &"get_pubkey".into());
        let _ = js_sys::Reflect::set(&msg, &"ok".into(), &true.into());
        let _ = js_sys::Reflect::set(&msg, &"result".into(), &pk.into());
        post_message(&msg.into());
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
    // Service loop: processes signer requests from parser and writes responses back.
    // Uses select! to race between from_parser and from_connections receivers.
    //
    // TODO: The to_main MessagePort parameter is currently UNUSED. All control responses
    // (bunker_discovered, pubkey, sign_event responses) still use postMessage() to the
    // main thread. The cryptoToMain MessageChannel was created but never wired up.
    // Either:
    //   1. Route control responses through to_main Port instead of post_message()
    //   2. Remove the to_main parameter entirely and use postMessage for everything
    //   3. Keep both: Port for structured control, postMessage for NIP-07 extension
    //
    // Currently option 3 is happening by accident - postMessage for everything.
    fn start_service_loop(
        &self,
        mut from_parser_rx: mpsc::Receiver<Vec<u8>>,
        mut from_connections_rx: mpsc::Receiver<Vec<u8>>,
        _to_main: Port, // UNUSED - see TODO above
    ) {
        let to_parser = self.to_parser.clone();
        let to_connections = self.to_connections.clone();
        let active = self.active.clone();

        spawn_local(async move {
            info!("[crypto] service loop started with MessageChannel");

            loop {
                // Use select! to race between the two receivers
                let bytes_opt: Option<Vec<u8>> = select! {
                    bytes_opt = from_parser_rx.next().fuse() => {
                        if bytes_opt.is_some() {
                            info!("[crypto] Received message from parser port");
                        } else {
                            info!("[crypto] Parser channel closed");
                        }
                        bytes_opt
                    }
                    bytes_opt = from_connections_rx.next().fuse() => {
                        if bytes_opt.is_some() {
                            info!("[crypto] Received message from connections port (NIP-46)");
                        } else {
                            info!("[crypto] Connections channel closed");
                        }
                        bytes_opt
                    }
                };

                // Break if both channels closed
                let bytes = match bytes_opt {
                    Some(b) => b,
                    None => {
                        info!("[crypto] All channels closed, exiting service loop");
                        break;
                    }
                };

                // Process the request (from either channel)
                // Note: NIP-46 responses are handled by the Pump in the Nip46Signer
                // This service loop handles SignerRequest from parser

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
                } else if op == shared::generated::nostr::fb::SignerOp::AuthEvent {
                    // AuthEvent: sign a kind 22242 authentication event for NIP-42
                    handle_auth_event(&active.borrow(), payload).await
                } else {
                    // Clone the active signer before async operations to avoid borrow conflicts
                    let active_signer = active.borrow().clone();

                    // Dispatch to active signer for signer-dependent operations
                    match active_signer {
                        ActiveSigner::Unset => Err("crypto not configured".into()),
                        ActiveSigner::Pk(pk) => {
                            match op {
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
                                    info!("[crypto] nip04_decrypt called with peer='{}', payload_len={}", peer, payload.len());
                                    let result =
                                        pk.nip04_decrypt(peer, payload).map_err(|e| e.to_string());
                                    info!(
                                        "[crypto] nip04_decrypt result: {:?}",
                                        if result.is_ok() { "ok" } else { "err" }
                                    );
                                    result
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
                            }
                        }
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
                        ActiveSigner::Nip46(s) => {
                            info!("[nip46] Service loop handling NIP-46 operation: {:?}", op);
                            match op {
                                shared::generated::nostr::fb::SignerOp::GetPubkey => {
                                    info!("[nip46] Getting public key from remote signer");
                                    let result = s.get_public_key().await;
                                    match &result {
                                        Ok(pk) => info!("[nip46] Got public key: {}", pk),
                                        Err(e) => {
                                            error!("[nip46] Failed to get public key: {:?}", e)
                                        }
                                    }
                                    result.map_err(|e| format!("{:?}", e))
                                }
                                shared::generated::nostr::fb::SignerOp::SignEvent => {
                                    info!("[nip46] Signing event via remote signer");
                                    match s.sign_event(payload).await {
                                        Ok(signed) => {
                                            info!("[nip46] Event signed successfully");
                                            Ok(serde_json::to_string(&signed)
                                                .unwrap_or_else(|_| "{}".to_string()))
                                        }
                                        Err(e) => {
                                            error!("[nip46] Failed to sign event: {:?}", e);
                                            Err(format!("{:?}", e))
                                        }
                                    }
                                }
                                shared::generated::nostr::fb::SignerOp::Nip04Encrypt => {
                                    info!("[nip46] NIP-04 encrypt via remote signer");
                                    match s.nip04_encrypt(peer, payload).await {
                                        Ok(out) => {
                                            info!("[nip46] NIP-04 encrypt successful");
                                            Ok(out)
                                        }
                                        Err(e) => {
                                            error!("[nip46] NIP-04 encrypt failed: {:?}", e);
                                            Err(format!("{:?}", e))
                                        }
                                    }
                                }
                                shared::generated::nostr::fb::SignerOp::Nip04Decrypt => {
                                    info!("[nip46] NIP-04 decrypt via remote signer");
                                    match s.nip04_decrypt(peer, payload).await {
                                        Ok(out) => {
                                            info!("[nip46] NIP-04 decrypt successful");
                                            Ok(out)
                                        }
                                        Err(e) => {
                                            error!("[nip46] NIP-04 decrypt failed: {:?}", e);
                                            Err(format!("{:?}", e))
                                        }
                                    }
                                }
                                shared::generated::nostr::fb::SignerOp::Nip44Encrypt => {
                                    info!("[nip46] NIP-44 encrypt via remote signer");
                                    match s.nip44_encrypt(peer, payload).await {
                                        Ok(out) => {
                                            info!("[nip46] NIP-44 encrypt successful");
                                            Ok(out)
                                        }
                                        Err(e) => {
                                            error!("[nip46] NIP-44 encrypt failed: {:?}", e);
                                            Err(format!("{:?}", e))
                                        }
                                    }
                                }
                                shared::generated::nostr::fb::SignerOp::Nip44Decrypt => {
                                    info!("[nip46] NIP-44 decrypt via remote signer");
                                    match s.nip44_decrypt(peer, payload).await {
                                        Ok(out) => {
                                            info!("[nip46] NIP-44 decrypt successful");
                                            Ok(out)
                                        }
                                        Err(e) => {
                                            error!("[nip46] NIP-44 decrypt failed: {:?}", e);
                                            Err(format!("{:?}", e))
                                        }
                                    }
                                }
                                shared::generated::nostr::fb::SignerOp::Nip04DecryptBetween => {
                                    info!("[nip46] NIP-04 decrypt between via remote signer");
                                    match s.nip04_decrypt_between(sender, recipient, payload).await
                                    {
                                        Ok(out) => {
                                            info!("[nip46] NIP-04 decrypt between successful");
                                            Ok(out)
                                        }
                                        Err(e) => {
                                            error!(
                                                "[nip46] NIP-04 decrypt between failed: {:?}",
                                                e
                                            );
                                            Err(format!("{:?}", e))
                                        }
                                    }
                                }
                                shared::generated::nostr::fb::SignerOp::Nip44DecryptBetween => {
                                    info!("[nip46] NIP-44 decrypt between via remote signer");
                                    match s.nip44_decrypt_between(sender, recipient, payload).await
                                    {
                                        Ok(out) => {
                                            info!("[nip46] NIP-44 decrypt between successful");
                                            Ok(out)
                                        }
                                        Err(e) => {
                                            error!(
                                                "[nip46] NIP-44 decrypt between failed: {:?}",
                                                e
                                            );
                                            Err(format!("{:?}", e))
                                        }
                                    }
                                }
                                _ => {
                                    warn!("[nip46] Unsupported operation requested");
                                    Err("Unsupported operation".to_string())
                                }
                            }
                        }
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

                // Send response: AuthEvent responses go to connections, others to parser
                let send_result = if op == shared::generated::nostr::fb::SignerOp::AuthEvent {
                    to_connections.borrow().send(out)
                } else {
                    to_parser.borrow().send(out)
                };
                if let Err(e) = send_result {
                    warn!("[crypto] failed to send SignerResponse: {:?}", e);
                }
            }

            info!("[crypto] service loop ended");
        });

        info!("[crypto] service loop spawned");
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

/// Handle AuthEvent operation: sign a kind 22242 authentication event for NIP-42
/// Payload JSON: {"challenge": "...", "relay": "wss://...", "created_at": 1234567890}
/// Returns JSON: {"event": "<signed_event_json>", "relay": "wss://..."}
async fn handle_auth_event(signer: &ActiveSigner, payload: &str) -> Result<String, String> {
    use serde_json::json;

    tracing::info!(payload, "handle_auth_event called");

    // Parse payload
    let auth_req: serde_json::Value =
        serde_json::from_str(payload).map_err(|e| format!("invalid auth request: {}", e))?;

    let challenge = auth_req["challenge"].as_str().ok_or("missing challenge")?;
    let relay_url = auth_req["relay"].as_str().ok_or("missing relay")?;
    let created_at = auth_req["created_at"]
        .as_u64()
        .unwrap_or_else(|| js_sys::Date::now() as u64);

    tracing::info!(
        relay = relay_url,
        challenge,
        created_at,
        "Building kind 22242 AUTH event"
    );

    // Build kind 22242 event template for NIP-42
    let template = json!({
        "kind": 22242,
        "created_at": created_at,
        "tags": [
            ["relay", relay_url],
            ["challenge", challenge]
        ],
        "content": ""
    });

    tracing::debug!(template = template.to_string(), "Event template");

    // Sign the event
    tracing::info!(signer_type = ?std::mem::discriminant(signer), "Signing with active signer");
    let signed_event = match signer {
        ActiveSigner::Unset => {
            tracing::error!("No signer configured for AuthEvent");
            return Err("no signer configured".into());
        }
        ActiveSigner::Pk(pk) => {
            tracing::info!("Signing AuthEvent with PrivateKey signer");
            pk.sign_event(&template.to_string())
                .await
                .map_err(|e| e.to_string())?
        }
        ActiveSigner::Nip07(nip07) => {
            tracing::info!("Signing AuthEvent with NIP-07 signer");
            let signed = nip07
                .sign_event(&template.to_string())
                .await
                .map_err(|e| format!("{:?}", e))?;
            serde_json::to_string(&signed).map_err(|e| e.to_string())?
        }
        ActiveSigner::Nip46(nip46) => {
            tracing::info!("Signing AuthEvent with NIP-46 signer");
            let signed = nip46
                .sign_event(&template.to_string())
                .await
                .map_err(|e| format!("{:?}", e))?;
            serde_json::to_string(&signed).map_err(|e| e.to_string())?
        }
    };

    tracing::info!(event_len = signed_event.len(), "Event signed successfully");
    tracing::debug!(event = signed_event, "Signed event content");

    // Return signed event with relay URL for routing
    let result = json!({
        "event": signed_event,
        "relay": relay_url
    });

    let result_str = result.to_string();
    tracing::info!(
        relay = relay_url,
        result_len = result_str.len(),
        "AuthEvent result ready"
    );

    Ok(result_str)
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
