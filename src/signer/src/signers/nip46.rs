use gloo_timers::future::TimeoutFuture;
use js_sys::Date;
use serde_json::{json, Value};
use shared::SabRing;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use tracing::{debug, info, warn};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::signers::{
    nip04, nip44,
    nip44::ConversationKey,
    types::{Keys, PublicKey, SecretKey},
};
use shared::types::{Event, EventId, UnsignedEvent};
use shared::utils::extract_first_three;
use signature::hazmat::PrehashVerifier;

use k256::schnorr::SigningKey;
use signature::hazmat::PrehashSigner;

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
    /// Expected secret for QR code validation (optional)
    pub expected_secret: Option<String>,
}

/// A complete NIP-46 client that:
/// - Manages an ephemeral client keypair
/// - Builds and signs kind 24133 requests
/// - Encrypts payloads with NIP-44 (preferred) or NIP-04
/// - Sends/receives over SAB frames via the connections worker
/// - Correlates JSON-RPC replies by id and resolves pending requests
pub struct Nip46Signer {
    cfg: Nip46Config,
    ws_req: Rc<RefCell<SabRing>>,
    ws_resp: Rc<RefCell<SabRing>>,

    // Ephemeral client keys
    client_keys: Keys,
    client_pubkey_hex: String,

    // REQ subscription id
    sub_id: String,

    // Incrementing id counter for RPC requests
    id_counter: Cell<u64>,

    // Background response pump guard
    pump_started: Cell<bool>,

    // Pending responses map: id -> result or error
    pending: Rc<RefCell<HashMap<String, Result<String, String>>>>,
    /// Cached user pubkey learned via get_public_key
    user_pubkey: Rc<RefCell<Option<String>>>,

    // Discovered remote signer pubkey (for QR code mode)
    discovered_remote_pubkey: Rc<RefCell<Option<String>>>,
}

impl Nip46Signer {
    /// Create a new NIP-46 signer over the given SAB rings.
    ///
    /// Generates a fresh ephemeral client keypair for each instance.
    pub fn new(
        cfg: Nip46Config,
        ws_req: Rc<RefCell<SabRing>>,
        ws_resp: Rc<RefCell<SabRing>>,
    ) -> Self {
        let client_keys = Keys::generate();
        let client_pubkey_hex = client_keys.public_key().to_hex();
        let sub_id = format!("n46:{}", &client_pubkey_hex);

        Self {
            cfg,
            ws_req,
            ws_resp,
            client_keys,
            client_pubkey_hex,
            sub_id,
            id_counter: Cell::new(Self::unix_time() as u64),
            pump_started: Cell::new(false),
            pending: Rc::new(RefCell::new(HashMap::new())),
            user_pubkey: Rc::new(RefCell::new(None)),
            discovered_remote_pubkey: Rc::new(RefCell::new(None)),
        }
    }

    /// Start the NIP-46 session:
    /// - open the REQ subscription for kind 24133 events addressed to this client
    /// - spawn the background response pump
    pub fn start(&self) {
        self.open_req_subscription();
        self.spawn_pump_once();
        info!(
            "[nip46] started (sub_id={}, client={})",
            self.sub_id, self.client_pubkey_hex
        );
    }

    /// Close the NIP-46 REQ subscription.
    pub fn close(&self) {
        self.send_close();
    }

    /// Get the discovered remote signer pubkey (for QR code mode).
    pub fn get_discovered_remote_pubkey(&self) -> Option<String> {
        self.discovered_remote_pubkey.borrow().clone()
    }

    // -------------------------
    // Public RPC-style methods
    // -------------------------

    /// RPC: request the remote signer's user public key.
    pub async fn get_public_key(&self) -> Result<String, JsValue> {
        // Return cached if present
        if let Some(pk) = self.user_pubkey.borrow().as_ref() {
            return Ok(pk.clone());
        }
        let id = self.next_id();
        let params: Vec<String> = vec![];
        let res = self.rpc_call("get_public_key", params, &id).await?;
        *self.user_pubkey.borrow_mut() = Some(res.clone());
        Ok(res)
    }

    /// RPC: ask the remote signer to sign the provided event template (JSON string).
    /// Returns the signed event as serde_json::Value.
    pub async fn sign_event(&self, template_json: &str) -> Result<serde_json::Value, JsValue> {
        // As per NIP-46, params is [json_stringified(template)]
        let id = self.next_id();
        let params: Vec<String> = vec![template_json.to_string()];
        let res = self.rpc_call("sign_event", params, &id).await?;

        // The result is json_stringified(<signed_event>)
        let v: Value =
            serde_json::from_str(&res).map_err(|e| JsValue::from_str(&format!("{e}")))?;
        Ok(v)
    }

    /// Optional utility: ping the remote signer, returns "pong"
    pub async fn ping(&self) -> Result<String, JsValue> {
        let id = self.next_id();
        self.rpc_call("ping", vec![], &id).await
    }

    /// RPC: NIP-04 encrypt plaintext for a third party pubkey (hex).
    pub async fn nip04_encrypt(
        &self,
        third_party_pubkey_hex: &str,
        plaintext: &str,
    ) -> Result<String, JsValue> {
        let id = self.next_id();
        let params = vec![third_party_pubkey_hex.to_string(), plaintext.to_string()];
        self.rpc_call("nip04_encrypt", params, &id).await
    }

    /// RPC: NIP-04 decrypt ciphertext from a third party pubkey (hex).
    pub async fn nip04_decrypt(
        &self,
        third_party_pubkey_hex: &str,
        ciphertext: &str,
    ) -> Result<String, JsValue> {
        let id = self.next_id();
        let params = vec![third_party_pubkey_hex.to_string(), ciphertext.to_string()];
        self.rpc_call("nip04_decrypt", params, &id).await
    }

    /// RPC: NIP-44 encrypt plaintext for a third party pubkey (hex).
    pub async fn nip44_encrypt(
        &self,
        third_party_pubkey_hex: &str,
        plaintext: &str,
    ) -> Result<String, JsValue> {
        let id = self.next_id();
        let params = vec![third_party_pubkey_hex.to_string(), plaintext.to_string()];
        self.rpc_call("nip44_encrypt", params, &id).await
    }

    /// RPC: NIP-44 decrypt ciphertext from a third party pubkey (hex).
    pub async fn nip44_decrypt(
        &self,
        third_party_pubkey_hex: &str,
        ciphertext: &str,
    ) -> Result<String, JsValue> {
        let id = self.next_id();
        let params = vec![third_party_pubkey_hex.to_string(), ciphertext.to_string()];
        self.rpc_call("nip44_decrypt", params, &id).await
    }

    /// RPC: NIP-04 decrypt when both participants are provided (sender/recipient).
    /// Chooses the correct peer based on cached user pubkey and delegates to nip04_decrypt.
    pub async fn nip04_decrypt_between(
        &self,
        sender_pubkey_hex: &str,
        recipient_pubkey_hex: &str,
        ciphertext: &str,
    ) -> Result<String, JsValue> {
        let upk = self.ensure_user_pubkey().await?;
        let peer_hex = if upk == sender_pubkey_hex {
            recipient_pubkey_hex
        } else {
            sender_pubkey_hex
        };
        self.nip04_decrypt(peer_hex, ciphertext).await
    }

    /// RPC: NIP-44 decrypt when both participants are provided (sender/recipient).
    /// Chooses the correct peer based on cached user pubkey and delegates to nip44_decrypt.
    pub async fn nip44_decrypt_between(
        &self,
        sender_pubkey_hex: &str,
        recipient_pubkey_hex: &str,
        ciphertext: &str,
    ) -> Result<String, JsValue> {
        let upk = self.ensure_user_pubkey().await?;
        let peer_hex = if upk == sender_pubkey_hex {
            recipient_pubkey_hex
        } else {
            sender_pubkey_hex
        };
        self.nip44_decrypt(peer_hex, ciphertext).await
    }

    /// Ensure we have the user pubkey cached, fetching via RPC if missing.
    async fn ensure_user_pubkey(&self) -> Result<String, JsValue> {
        if let Some(pk) = self.user_pubkey.borrow().as_ref() {
            return Ok(pk.clone());
        }
        let pk = self.get_public_key().await?;
        Ok(pk)
    }

    // ------------------------
    // Core RPC send/receive
    // ------------------------

    async fn rpc_call(
        &self,
        method: &str,
        params: Vec<String>,
        id: &str,
    ) -> Result<String, JsValue> {
        // Build JSON-RPC-like payload (strings only for params)
        let payload = json!({
            "id": id,
            "method": method,
            "params": params,
        })
        .to_string();

        // Encrypt payload
        let encrypted = self.encrypt_for_remote(&payload)?;

        // Build and publish NIP-46 request event (kind 24133)
        self.publish_nip46_event(&encrypted)?;

        // Await response by id
        let result = self.await_response(id, 20_000).await?; // 20s timeout
        Ok(result)
    }

    async fn await_response(&self, id: &str, timeout_ms: u32) -> Result<String, JsValue> {
        let start = Self::unix_time_ms();
        let mut sleep_ms: u32 = 8;
        let max_sleep: u32 = 256;

        loop {
            // Check resolved map
            if let Some(done) = self.pending.borrow_mut().remove(id) {
                match done {
                    Ok(s) => return Ok(s),
                    Err(e) => return Err(JsValue::from_str(&format!("nip46 error: {}", e))),
                }
            }

            // Timeout?
            let now = Self::unix_time_ms();
            if (now - start) > timeout_ms as f64 {
                return Err(JsValue::from_str("nip46 timeout waiting for response"));
            }

            TimeoutFuture::new(sleep_ms).await;
            sleep_ms = (sleep_ms * 2).min(max_sleep);
        }
    }

    fn encrypt_for_remote(&self, plaintext: &str) -> Result<String, JsValue> {
        // Prefer NIP-44, fall back to NIP-04 if disabled or fails
        let remote_pk =
            PublicKey::from_hex(&self.cfg.remote_signer_pubkey).map_err(js_err_from_types)?;
        let secret = self
            .client_keys
            .secret_key()
            .map_err(|e| JsValue::from_str(&format!("secret key: {}", e)))?;

        if self.cfg.use_nip44 {
            let conv = ConversationKey::derive(secret, &remote_pk)
                .map_err(|e| JsValue::from_str(&format!("nip44 derive: {}", e)))?;
            match nip44::encrypt(plaintext, &conv) {
                Ok(ct) => return Ok(ct),
                Err(e) => {
                    warn!("[nip46] nip44 encrypt failed, trying nip04: {}", e);
                }
            }
        }

        // NIP-04 fallback
        nip04::encrypt(secret, &remote_pk, plaintext)
            .map_err(|e| JsValue::from_str(&format!("nip04 encrypt: {}", e)))
    }

    fn decrypt_from_remote(&self, ciphertext: &str) -> Result<String, JsValue> {
        // Prefer NIP-44, fall back to NIP-04
        let remote_pk =
            PublicKey::from_hex(&self.cfg.remote_signer_pubkey).map_err(js_err_from_types)?;
        let secret = self
            .client_keys
            .secret_key()
            .map_err(|e| JsValue::from_str(&format!("secret key: {}", e)))?;

        if self.cfg.use_nip44 {
            let conv = ConversationKey::derive(secret, &remote_pk)
                .map_err(|e| JsValue::from_str(&format!("nip44 derive: {}", e)))?;
            match nip44::decrypt(ciphertext, &conv) {
                Ok(pt) => return Ok(pt),
                Err(e) => {
                    warn!("[nip46] nip44 decrypt failed, trying nip04: {}", e);
                }
            }
        }

        nip04::decrypt(secret, &remote_pk, ciphertext)
            .map_err(|e| JsValue::from_str(&format!("nip04 decrypt: {}", e)))
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

    /// Build, sign and publish a NIP-46 request event.
    fn publish_nip46_event(&self, encrypted_content: &str) -> Result<(), JsValue> {
        // Use discovered remote pubkey if available, otherwise use the configured one
        let remote_pubkey =
            if let Some(discovered) = self.discovered_remote_pubkey.borrow().as_ref() {
                discovered.clone()
            } else {
                self.cfg.remote_signer_pubkey.clone()
            };

        let mut tags = vec![vec!["p".to_string(), remote_pubkey]];
        if let Some(app) = &self.cfg.app_name {
            tags.push(vec!["client".to_string(), app.clone()]);
        }

        let created_at = Self::unix_time();
        let kind: u32 = 24133;

        // Create an UnsignedEvent and convert to Event
        let unsigned_event = UnsignedEvent::new(
            &self.client_pubkey_hex,
            kind as u16,
            encrypted_content.to_string(),
            tags,
        )
        .map_err(|e| JsValue::from_str(&format!("Failed to create unsigned event: {}", e)))?;

        // Convert to Event
        let mut event = Event {
            id: EventId([0u8; 32]),
            pubkey: unsigned_event.pubkey,
            created_at: created_at as u64,
            kind: unsigned_event.kind,
            tags: unsigned_event.tags,
            content: unsigned_event.content,
            sig: String::new(),
        };

        // Compute and set the event ID
        event
            .compute_id()
            .map_err(|e| JsValue::from_str(&format!("Failed to compute event ID: {}", e)))?;

        // Sign the event
        let secret_key = self
            .client_keys
            .secret_key()
            .map_err(|e| JsValue::from_str(&format!("Failed to get secret key: {}", e)))?;

        let signing_key = SigningKey::from_bytes(&secret_key.0)
            .map_err(|e| JsValue::from_str(&format!("Failed to create signing key: {}", e)))?;

        let verifying_key = signing_key.verifying_key();

        // Sign the event ID
        let signature = signing_key
            .sign_prehash(&event.id.to_bytes())
            .map_err(|e| JsValue::from_str(&format!("Schnorr prehash sign failed: {}", e)))?;

        // Verify with the prehash verifier to match nostr-tools/relay behavior
        verifying_key
            .verify_prehash(&event.id.to_bytes(), &signature)
            .map_err(|e| {
                JsValue::from_str(&format!("Local Schnorr prehash verify failed: {}", e))
            })?;

        // Set the signature on the event
        event.sig = hex::encode(signature.to_bytes());

        let frame = format!(r#"["EVENT",{}]"#, event.as_json());
        self.publish_frames(&[frame]);
        Ok(())
    }

    /// Publish frames using the Envelope format expected by the connections worker
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

    /// Background pump to drain ws_response_signer:
    /// - parse incoming frames
    /// - filter NIP-46 responses (kind 24133 addressed to us)
    /// - decrypt and correlate by JSON-RPC id
    fn spawn_pump_once(&self) {
        if self.pump_started.get() {
            return;
        }
        self.pump_started.set(true);

        // Clone all necessary data to avoid borrowing issues - NetworkManager pattern
        let ws_resp = self.ws_resp.clone();
        let sub_id = self.sub_id.clone();
        let remote_pk_str = self.cfg.remote_signer_pubkey.clone();
        let pending = self.pending.clone();
        let discovered_remote_pubkey = self.discovered_remote_pubkey.clone();
        let client_pk = self.client_pubkey_hex.clone();

        // Clone config fields that will be needed in the closure
        let expected_secret = self.cfg.expected_secret.clone();

        // Move keys into the closure to avoid lifetime issues
        let secret_bytes = self.client_keys.secret_key.0;
        let use_nip44 = self.cfg.use_nip44;

        // Create a closure that captures the cloned data instead of self
        let pump_task = async move {
            let remote_pk_str_for_closure = remote_pk_str.clone();

            let decrypt_helper = move |cipher: &str| -> Result<String, String> {
                // Prefer nip44 then nip04, similar to decrypt_from_remote
                let remote_pk = PublicKey::from_hex(&remote_pk_str_for_closure)
                    .map_err(|e| format!("pk: {}", e))?;
                let keys = Keys::new(SecretKey(secret_bytes));
                let secret = &keys.secret_key;

                if use_nip44 {
                    let conv = ConversationKey::derive(secret, &remote_pk)
                        .map_err(|e| format!("nip44 derive: {}", e))?;
                    match nip44::decrypt(cipher, &conv) {
                        Ok(pt) => return Ok(pt),
                        Err(e) => {
                            debug!("[nip46] pump nip44 decrypt failed, trying nip04: {}", e);
                        }
                    }
                }
                nip04::decrypt(secret, &remote_pk, cipher)
                    .map_err(|e| format!("nip04 decrypt: {}", e))
            };

            Self::run_pump_loop(
                ws_resp,
                sub_id,
                remote_pk_str,
                pending,
                discovered_remote_pubkey,
                client_pk,
                expected_secret,
                decrypt_helper,
            )
            .await;
        };

        // Spawn the task with the cloned data
        spawn_local(pump_task);

        info!("[nip46] response pump started");
    }

    async fn run_pump_loop(
        ws_resp: Rc<RefCell<SabRing>>,
        sub_id: String,
        remote_pk_str: String,
        pending: Rc<RefCell<HashMap<String, Result<String, String>>>>,
        discovered_remote_pubkey: Rc<RefCell<Option<String>>>,
        client_pk: String,
        expected_secret: Option<String>,
        decrypt_helper: impl Fn(&str) -> Result<String, String>,
    ) {
        let mut sleep_ms: u32 = 8;
        let max_sleep: u32 = 256;

        loop {
            let maybe = { ws_resp.borrow_mut().read_next() };

            if let Some(bytes) = maybe {
                sleep_ms = 8;

                // Process the frame using NetworkManager-style pattern
                Self::handle_nip46_frame(
                    &bytes,
                    &sub_id,
                    &remote_pk_str,
                    &pending,
                    &discovered_remote_pubkey,
                    &client_pk,
                    &expected_secret,
                    &decrypt_helper,
                )
                .await;

                continue;
            }

            TimeoutFuture::new(sleep_ms).await;
            sleep_ms = (sleep_ms * 2).min(max_sleep);
        }
    }

    async fn handle_nip46_frame(
        bytes: &[u8],
        sub_id: &str,
        remote_pk_str: &str,
        pending: &Rc<RefCell<HashMap<String, Result<String, String>>>>,
        discovered_remote_pubkey: &Rc<RefCell<Option<String>>>,
        client_pk: &str,
        expected_secret: &Option<String>,
        decrypt_helper: &impl Fn(&str) -> Result<String, String>,
    ) {
        // Parse frame using NetworkManager-style pattern matching
        match std::str::from_utf8(bytes) {
            Ok(s) => {
                let prev = preview(s, 200);
                debug!(
                    "[nip46] ws_response {} bytes (utf8 preview): {}",
                    bytes.len(),
                    prev
                );

                // Parse JSON array frame and match on message type
                match extract_first_three(s) {
                    Some([first, second, third]) => match first {
                        Some("\"EVENT\"") | Some("\"event\"") => {
                            Self::handle_nip46_event(
                                second,
                                third,
                                sub_id,
                                remote_pk_str,
                                pending,
                                discovered_remote_pubkey,
                                client_pk,
                                expected_secret,
                                decrypt_helper,
                            )
                            .await;
                        }
                        Some("\"OK\"") | Some("\"ok\"") => {
                            debug!("[nip46] Received OK response");
                        }
                        Some("\"NOTICE\"") | Some("\"notice\"") => {
                            debug!(
                                "[nip46] Received notice: {} {}",
                                second.unwrap_or(""),
                                third.unwrap_or("")
                            );
                        }
                        Some("\"ERROR\"") | Some("\"error\"") => {
                            warn!(
                                "[nip46] Received error: {} {}",
                                second.unwrap_or(""),
                                third.unwrap_or("")
                            );
                        }
                        _ => {
                            debug!("[nip46] Unknown message type: {:?}", first);
                        }
                    },
                    None => {
                        warn!("[nip46] Failed to parse JSON array frame");
                    }
                }
            }
            Err(_) => {
                debug!("[nip46] ws_response {} binary bytes", bytes.len());
            }
        }
    }

    async fn handle_nip46_event(
        second: Option<&str>,
        third: Option<&str>,
        sub_id: &str,
        remote_pk_str: &str,
        pending: &Rc<RefCell<HashMap<String, Result<String, String>>>>,
        discovered_remote_pubkey: &Rc<RefCell<Option<String>>>,
        client_pk: &str,
        expected_secret: &Option<String>,
        decrypt_helper: &impl Fn(&str) -> Result<String, String>,
    ) {
        let (maybe_sub, evt_json) = match (second, third) {
            (Some(sub), Some(evt)) => (Some(sub), evt),
            (None, Some(evt)) => (None, evt),
            _ => {
                // Invalid format, skip
                return;
            }
        };

        // Check subscription ID if present
        if let Some(sub_str) = maybe_sub {
            // Remove quotes from subscription string
            let sub_id_clean = sub_str.trim_matches('"');
            if sub_id_clean != sub_id {
                // not for our subscription
                return;
            }
        }

        // Parse the event JSON using Event::from_json
        if let Ok(event) = Event::from_json(evt_json) {
            // Validate this is a NIP-46 event (kind 24133)
            if event.kind() != 24133 {
                return;
            }

            // Check "pubkey" == remote signer (best-effort)
            let event_pubkey = event.pubkey.to_hex();
            if event_pubkey != remote_pk_str {
                // could be middleware/proxy, don't drop strictly
                debug!("[nip46] event from unexpected pubkey: {}", event_pubkey);
            }

            // Ensure the p-tag targets us
            let mut addressed_to_us = false;
            for tag in event.tags() {
                if tag.get(0) == Some(&"p".to_string())
                    && tag.get(1) == Some(&client_pk.to_string())
                {
                    addressed_to_us = true;
                    break;
                }
            }
            if !addressed_to_us {
                return;
            }

            // Decrypt content and process RPC response
            let ciphertext = event.content();
            if let Ok(pt) = decrypt_helper(ciphertext) {
                Self::process_rpc_response(
                    &pt,
                    &event_pubkey,
                    pending,
                    discovered_remote_pubkey,
                    expected_secret,
                );
            } else {
                warn!("[nip46] decrypt failed");
            }
        }
    }

    fn process_rpc_response(
        plaintext: &str,
        event_pubkey: &str,
        pending: &Rc<RefCell<HashMap<String, Result<String, String>>>>,
        discovered_remote_pubkey: &Rc<RefCell<Option<String>>>,
        expected_secret: &Option<String>,
    ) {
        if let Ok(rpc) = serde_json::from_str::<Value>(plaintext) {
            let rid = rpc
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();

            let err = rpc
                .get("error")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let res = rpc
                .get("result")
                .and_then(|v| {
                    if v.is_string() {
                        v.as_str().map(|s| s.to_string())
                    } else {
                        // If it's not a string, stringify it
                        Some(v.to_string())
                    }
                })
                .unwrap_or_default();

            // Process the response
            let outcome = if let Some(e) = err {
                Err(e)
            } else {
                // Check if we're in QR code mode and need to validate the secret
                if let Some(expected_secret) = expected_secret {
                    if let Some(result) = rpc.get("result") {
                        if let Some(result_str) = result.as_str() {
                            if result_str == expected_secret {
                                // This is a valid discovery response!
                                let remote_pubkey = event_pubkey;

                                // Update our configuration with the discovered remote pubkey
                                info!("[nip46] Remote signer discovered: {}", remote_pubkey);
                                *discovered_remote_pubkey.borrow_mut() =
                                    Some(remote_pubkey.to_string());

                                // Continue with normal processing
                                Ok(res)
                            } else {
                                // Secret doesn't match - this is not a valid response
                                Err("Invalid secret in response".to_string())
                            }
                        } else {
                            Err("Invalid result format".to_string())
                        }
                    } else {
                        Err("No result in response".to_string())
                    }
                } else {
                    // Normal processing (not in QR code mode)
                    Ok(res)
                }
            };

            pending.borrow_mut().insert(rid, outcome);
        }
    }

    // ---------------
    // Event utilities
    // ---------------

    // compute_event_id and sign_event_id methods are now replaced by using the Event struct

    // ----------------
    // Utility helpers
    // ----------------

    fn next_id(&self) -> String {
        let c = self.id_counter.get().wrapping_add(1);
        self.id_counter.set(c);
        // Short random-ish string: counter + ms timestamp
        format!("{}-{}", c, Self::unix_time_ms() as u64)
    }

    /// Return Unix time in seconds (from JS Date.now()).
    fn unix_time() -> u32 {
        (Date::now() / 1000f64) as u32
    }

    /// Return Unix time in milliseconds (from JS Date.now()).
    fn unix_time_ms() -> f64 {
        Date::now()
    }
}

fn preview(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…(+{} bytes)", &s[..max], s.len() - max)
    }
}

fn js_err_from_types(e: crate::signers::types::TypesError) -> JsValue {
    JsValue::from_str(&format!("{e}"))
}
