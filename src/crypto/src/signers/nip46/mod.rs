use gloo_timers::future::TimeoutFuture;
use js_sys::Date;
use serde_json::{json, Value};
use shared::types::Keys;
use shared::SabRing;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use tracing::{debug, info};
use wasm_bindgen::prelude::*;

pub mod config;
pub mod crypto;
pub mod pump;
pub mod transport;

pub use config::Nip46Config;
use crypto::Crypto;
use pump::Pump;
use transport::Transport;

/// A complete NIP-46 client refactored into modules.
pub struct Nip46Signer {
    cfg: Nip46Config,
    client_keys: Keys,
    client_pubkey_hex: String,
    sub_id: String,
    id_counter: Cell<u64>,
    pump_started: Cell<bool>,
    pending: Rc<RefCell<HashMap<String, Result<String, String>>>>,
    user_pubkey: Rc<RefCell<Option<String>>>,
    discovered_remote_pubkey: Rc<RefCell<Option<String>>>,

    // Sub-modules
    crypto: Crypto,
    transport: Transport,
    ws_resp: Rc<RefCell<SabRing>>,
    on_discovery: Rc<RefCell<Option<Rc<dyn Fn(String)>>>>,
}

impl Nip46Signer {
    pub fn new(
        cfg: Nip46Config,
        ws_req: Rc<RefCell<SabRing>>,
        ws_resp: Rc<RefCell<SabRing>>,
        client_keys: Option<Keys>,
    ) -> Self {
        let client_keys = client_keys.unwrap_or_else(Keys::generate);
        let client_pubkey_hex = client_keys.public_key().to_hex();
        // NIP-01 subscription IDs are often limited to 64 chars by relays.
        // "n46:" (4) + 60 chars of pubkey = 64 chars.
        let sub_id = format!("n46:{}", &client_pubkey_hex[..60]);

        let crypto = Crypto::new(
            client_keys.clone(),
            cfg.remote_signer_pubkey.clone(),
            cfg.use_nip44,
        );

        let transport = Transport::new(
            ws_req,
            cfg.relays.clone(),
            cfg.app_name.clone(),
            client_keys.clone(),
        );

        Self {
            cfg,
            client_keys,
            client_pubkey_hex,
            sub_id,
            id_counter: Cell::new(Self::unix_time() as u64),
            pump_started: Cell::new(false),
            pending: Rc::new(RefCell::new(HashMap::new())),
            user_pubkey: Rc::new(RefCell::new(None)),
            discovered_remote_pubkey: Rc::new(RefCell::new(None)),
            crypto,
            transport,
            ws_resp,
            on_discovery: Rc::new(RefCell::new(None)),
        }
    }

    pub fn start(&self, on_discovery: Option<Rc<dyn Fn(String)>>) {
        *self.on_discovery.borrow_mut() = on_discovery;
        self.transport
            .open_req_subscription(&self.sub_id, Self::unix_time());
        self.spawn_pump_once();
        info!(
            "[nip46] started (sub_id={}, client={})",
            self.sub_id, self.client_pubkey_hex
        );
    }

    pub fn close(&self) {
        self.transport.send_close(&self.sub_id);
    }

    pub fn get_discovered_remote_pubkey(&self) -> Option<String> {
        self.discovered_remote_pubkey.borrow().clone()
    }

    pub fn get_bunker_url(&self) -> Option<String> {
        let remote_pk = self.discovered_remote_pubkey.borrow().clone()?;
        let mut url = format!("bunker://{}?", remote_pk);
        for (i, relay) in self.cfg.relays.iter().enumerate() {
            if i > 0 {
                url.push('&');
            }
            let encoded_relay = js_sys::encode_uri_component(relay);
            url.push_str(&format!("relay={}", String::from(encoded_relay)));
        }
        if let Some(secret) = &self.cfg.expected_secret {
            url.push_str(&format!("&secret={}", secret));
        }
        Some(url)
    }

    pub async fn connect(&self) -> Result<String, JsValue> {
        let id = self.next_id();
        let mut params = vec![self.cfg.remote_signer_pubkey.clone()];
        if let Some(secret) = &self.cfg.expected_secret {
            params.push(secret.clone());
        }
        self.rpc_call("connect", params, &id).await
    }

    pub async fn get_public_key(&self) -> Result<String, JsValue> {
        if let Some(pk) = self.user_pubkey.borrow().as_ref() {
            return Ok(pk.clone());
        }
        let id = self.next_id();
        let res = self.rpc_call("get_public_key", vec![], &id).await?;
        *self.user_pubkey.borrow_mut() = Some(res.clone());
        Ok(res)
    }

    pub async fn sign_event(&self, template_json: &str) -> Result<serde_json::Value, JsValue> {
        let id = self.next_id();
        let params = vec![template_json.to_string()];
        let res = self.rpc_call("sign_event", params, &id).await?;
        let v: Value =
            serde_json::from_str(&res).map_err(|e| JsValue::from_str(&format!("{e}")))?;
        Ok(v)
    }

    pub async fn ping(&self) -> Result<String, JsValue> {
        let id = self.next_id();
        self.rpc_call("ping", vec![], &id).await
    }

    pub async fn nip04_encrypt(
        &self,
        third_party_pubkey_hex: &str,
        plaintext: &str,
    ) -> Result<String, JsValue> {
        let id = self.next_id();
        let params = vec![third_party_pubkey_hex.to_string(), plaintext.to_string()];
        self.rpc_call("nip04_encrypt", params, &id).await
    }

    pub async fn nip04_decrypt(
        &self,
        third_party_pubkey_hex: &str,
        ciphertext: &str,
    ) -> Result<String, JsValue> {
        let id = self.next_id();
        let params = vec![third_party_pubkey_hex.to_string(), ciphertext.to_string()];
        self.rpc_call("nip04_decrypt", params, &id).await
    }

    pub async fn nip44_encrypt(
        &self,
        third_party_pubkey_hex: &str,
        plaintext: &str,
    ) -> Result<String, JsValue> {
        let id = self.next_id();
        let params = vec![third_party_pubkey_hex.to_string(), plaintext.to_string()];
        self.rpc_call("nip44_encrypt", params, &id).await
    }

    pub async fn nip44_decrypt(
        &self,
        third_party_pubkey_hex: &str,
        ciphertext: &str,
    ) -> Result<String, JsValue> {
        let id = self.next_id();
        let params = vec![third_party_pubkey_hex.to_string(), ciphertext.to_string()];
        self.rpc_call("nip44_decrypt", params, &id).await
    }

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

    async fn ensure_user_pubkey(&self) -> Result<String, JsValue> {
        if let Some(pk) = self.user_pubkey.borrow().as_ref() {
            return Ok(pk.clone());
        }
        self.get_public_key().await
    }

    async fn rpc_call(
        &self,
        method: &str,
        params: Vec<String>,
        id: &str,
    ) -> Result<String, JsValue> {
        let payload = json!({
            "id": id,
            "method": method,
            "params": params,
        })
        .to_string();

        let encrypted = self.crypto.encrypt_for_remote(&payload)?;

        let remote_pubkey =
            if let Some(discovered) = self.discovered_remote_pubkey.borrow().as_ref() {
                discovered.clone()
            } else {
                self.cfg.remote_signer_pubkey.clone()
            };

        self.transport
            .publish_nip46_event(&encrypted, &remote_pubkey, Self::unix_time())?;

        self.await_response(id, 20_000).await
    }

    async fn await_response(&self, id: &str, timeout_ms: u32) -> Result<String, JsValue> {
        let start = Self::unix_time_ms();
        let mut sleep_ms: u32 = 8;
        let max_sleep: u32 = 256;

        loop {
            if let Some(done) = self.pending.borrow_mut().remove(id) {
                match done {
                    Ok(s) => return Ok(s),
                    Err(e) => return Err(JsValue::from_str(&format!("nip46 error: {}", e))),
                }
            }

            let now = Self::unix_time_ms();
            if (now - start) > timeout_ms as f64 {
                return Err(JsValue::from_str("nip46 timeout waiting for response"));
            }

            TimeoutFuture::new(sleep_ms).await;
            sleep_ms = (sleep_ms * 2).min(max_sleep);
        }
    }

    fn spawn_pump_once(&self) {
        if self.pump_started.get() {
            return;
        }
        self.pump_started.set(true);

        Pump::spawn(
            self.ws_resp.clone(),
            self.sub_id.clone(),
            self.cfg.remote_signer_pubkey.clone(),
            self.pending.clone(),
            self.discovered_remote_pubkey.clone(),
            self.client_pubkey_hex.clone(),
            self.cfg.expected_secret.clone(),
            self.client_keys.clone(),
            self.cfg.use_nip44,
            self.on_discovery.clone(),
        );
    }

    fn next_id(&self) -> String {
        let c = self.id_counter.get().wrapping_add(1);
        self.id_counter.set(c);
        format!("{}-{}", c, Self::unix_time_ms() as u64)
    }

    fn unix_time() -> u32 {
        (Date::now() / 1000f64) as u32
    }

    fn unix_time_ms() -> f64 {
        Date::now()
    }
}
