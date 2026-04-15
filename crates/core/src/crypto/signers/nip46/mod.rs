use serde_json::{json, Value};
use crate::types::Keys;
use crate::port::Port;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use tracing::{debug, error, info};

use futures::channel::mpsc;

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
    crypto: RefCell<Crypto>,
    transport: Transport,
    from_connections_rx: Rc<RefCell<Option<mpsc::Receiver<Vec<u8>>>>>,
    on_discovery: Rc<RefCell<Option<Rc<dyn Fn(String)>>>>,
}

impl Nip46Signer {
    pub fn new(
        cfg: Nip46Config,
        to_connections: Rc<RefCell<dyn Port>>,
        from_connections_rx: mpsc::Receiver<Vec<u8>>,
        client_keys: Option<Keys>,
    ) -> Self {
        let client_keys = client_keys.unwrap_or_else(Keys::generate);
        let client_pubkey_hex = client_keys.public_key().to_hex();
        let sub_id = format!("n46:{}", &client_pubkey_hex[..60.min(client_pubkey_hex.len())]);

        let crypto = Crypto::new(
            client_keys.clone(),
            cfg.remote_signer_pubkey.clone(),
            cfg.use_nip44,
        );

        let transport = Transport::new(
            to_connections,
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
            crypto: RefCell::new(crypto),
            transport,
            from_connections_rx: Rc::new(RefCell::new(Some(from_connections_rx))),
            on_discovery: Rc::new(RefCell::new(None)),
        }
    }
    
    /// Get the client pubkey hex (for debug logging)
    pub fn get_client_pubkey(&self) -> &str {
        &self.client_pubkey_hex
    }
    
    /// Get the subscription ID (for debug logging)
    pub fn get_sub_id(&self) -> &str {
        &self.sub_id
    }
    
    /// Get the client secret key as hex string (for session storage)
    pub fn get_client_secret(&self) -> Result<String, String> {
        let secret = self.client_keys.secret_key()
            .map_err(|e| format!("Failed to get secret key: {}", e))?;
        Ok(hex::encode(secret.0))
    }

    pub fn start<F>(&self, spawner: F, on_discovery: Option<Rc<dyn Fn(String)>>)
    where
        F: Fn(std::pin::Pin<Box<dyn std::future::Future<Output = ()>>>) + 'static,
    {
        *self.on_discovery.borrow_mut() = on_discovery;
        self.transport.open_req_subscription(&self.sub_id, Self::unix_time());
        self.spawn_pump_once(spawner);
    }

    pub fn close(&self) {
        self.transport.send_close(&self.sub_id);
    }

    pub fn get_discovered_remote_pubkey(&self) -> Option<String> {
        let pk = self.discovered_remote_pubkey.borrow().clone();
        match &pk {
            Some(p) => debug!("[nip46] Discovered remote pubkey: {}", p),
            None => debug!("[nip46] No remote pubkey discovered yet"),
        }
        pk
    }

    /// Update the crypto module with the discovered remote signer pubkey
    pub fn update_crypto_remote_pubkey(&self, pubkey: &str) {
        info!("[nip46] Updating crypto remote pubkey: {}...", &pubkey[..16.min(pubkey.len())]);
        self.crypto.borrow_mut().set_remote_signer_pubkey(pubkey.to_string());
        info!("[nip46] Crypto remote pubkey updated successfully");
    }

    pub fn get_bunker_url(&self) -> Option<String> {
        let remote_pk = self.discovered_remote_pubkey.borrow().clone()?;
        let mut url = format!("bunker://{}?", remote_pk);
        for (i, relay) in self.cfg.relays.iter().enumerate() {
            if i > 0 {
                url.push('&');
            }
            let encoded_relay: String = url::form_urlencoded::byte_serialize(relay.as_bytes()).collect();
            url.push_str(&format!("relay={}", encoded_relay));
        }
        if let Some(secret) = &self.cfg.expected_secret {
            url.push_str(&format!("&secret={}", secret));
        }
        debug!("[nip46] Generated bunker URL: {}", url);
        Some(url)
    }

    pub async fn connect(&self) -> Result<String, String> {
        let id = self.next_id();
        let mut params = vec![self.cfg.remote_signer_pubkey.clone()];
        if let Some(secret) = &self.cfg.expected_secret {
            params.push(secret.clone());
        }
        self.rpc_call("connect", params, &id).await
    }

    pub async fn get_public_key(&self) -> Result<String, String> {
        if let Some(pk) = self.user_pubkey.borrow().as_ref() {
            return Ok(pk.clone());
        }
        let id = self.next_id();
        let res = self.rpc_call("get_public_key", vec![], &id).await?;
        *self.user_pubkey.borrow_mut() = Some(res.clone());
        Ok(res)
    }

    pub async fn sign_event(&self, template_json: &str) -> Result<Value, String> {
        let id = self.next_id();
        let params = vec![template_json.to_string()];
        let res = self.rpc_call("sign_event", params, &id).await?;
        let v: Value =
            serde_json::from_str(&res).map_err(|e| format!("{}", e))?;
        Ok(v)
    }

    pub async fn ping(&self) -> Result<String, String> {
        let id = self.next_id();
        self.rpc_call("ping", vec![], &id).await
    }

    pub async fn nip04_encrypt(
        &self,
        third_party_pubkey_hex: &str,
        plaintext: &str,
    ) -> Result<String, String> {
        let id = self.next_id();
        let params = vec![third_party_pubkey_hex.to_string(), plaintext.to_string()];
        self.rpc_call("nip04_encrypt", params, &id).await
    }

    pub async fn nip04_decrypt(
        &self,
        third_party_pubkey_hex: &str,
        ciphertext: &str,
    ) -> Result<String, String> {
        let id = self.next_id();
        let params = vec![third_party_pubkey_hex.to_string(), ciphertext.to_string()];
        self.rpc_call("nip04_decrypt", params, &id).await
    }

    pub async fn nip44_encrypt(
        &self,
        third_party_pubkey_hex: &str,
        plaintext: &str,
    ) -> Result<String, String> {
        let id = self.next_id();
        let params = vec![third_party_pubkey_hex.to_string(), plaintext.to_string()];
        self.rpc_call("nip44_encrypt", params, &id).await
    }

    pub async fn nip44_decrypt(
        &self,
        third_party_pubkey_hex: &str,
        ciphertext: &str,
    ) -> Result<String, String> {
        let id = self.next_id();
        let params = vec![third_party_pubkey_hex.to_string(), ciphertext.to_string()];
        self.rpc_call("nip44_decrypt", params, &id).await
    }

    pub async fn nip04_decrypt_between(
        &self,
        sender_pubkey_hex: &str,
        recipient_pubkey_hex: &str,
        ciphertext: &str,
    ) -> Result<String, String> {
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
    ) -> Result<String, String> {
        let upk = self.ensure_user_pubkey().await?;
        let peer_hex = if upk == sender_pubkey_hex {
            recipient_pubkey_hex
        } else {
            sender_pubkey_hex
        };
        self.nip44_decrypt(peer_hex, ciphertext).await
    }

    async fn ensure_user_pubkey(&self) -> Result<String, String> {
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
    ) -> Result<String, String> {
        info!("[nip46] rpc_call: method={}, id={}", method, id);
        
        let payload = json!({
            "id": id,
            "method": method,
            "params": params,
        })
        .to_string();
        debug!("[nip46] rpc_call: payload={}", payload);

        info!("[nip46] rpc_call: encrypting payload");
        let encrypted = self.crypto.borrow().encrypt_for_remote(&payload)?;
        debug!("[nip46] rpc_call: encrypted length={}", encrypted.len());

        let remote_pubkey =
            if let Some(discovered) = self.discovered_remote_pubkey.borrow().as_ref() {
                info!("[nip46] rpc_call: using discovered remote pubkey: {}", discovered);
                discovered.clone()
            } else {
                info!("[nip46] rpc_call: using configured remote pubkey: {}", 
                      if self.cfg.remote_signer_pubkey.is_empty() { "(empty, QR mode)".to_string() } 
                      else { self.cfg.remote_signer_pubkey.clone() });
                self.cfg.remote_signer_pubkey.clone()
            };

        self.transport
            .publish_nip46_event(&encrypted, &remote_pubkey, Self::unix_time())?;

        self.await_response(id, 20_000).await
    }

    async fn await_response(&self, id: &str, timeout_ms: u32) -> Result<String, String> {
        let start = Self::unix_time_ms();
        let mut polls = 0u32;
        let mut sleep_ms: u32 = 8;
        let max_sleep: u32 = 256;

        loop {
            if let Some(done) = self.pending.borrow_mut().remove(id) {
                match done {
                    Ok(s) => return Ok(s),
                    Err(e) => return Err(format!("nip46 error: {}", e)),
                }
            }

            let elapsed = Self::unix_time_ms() - start;
            if elapsed > timeout_ms as f64 {
                return Err("nip46 timeout waiting for response".to_string());
            }

            polls += 1;
            if polls >= sleep_ms {
                polls = 0;
                sleep_ms = (sleep_ms * 2).min(max_sleep);
            }
            futures::future::poll_fn(|_| std::task::Poll::Ready(())).await;
        }
    }

    fn spawn_pump_once<F>(&self, spawner: F)
    where
        F: Fn(std::pin::Pin<Box<dyn std::future::Future<Output = ()>>>) + 'static,
    {
        if self.pump_started.get() {
            return;
        }
        self.pump_started.set(true);

        if let Some(from_connections_rx) = self.from_connections_rx.borrow_mut().take() {
            Pump::spawn(
                spawner,
                from_connections_rx,
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
        } else {
            error!("[nip46] Pump receiver already taken");
        }
    }

    fn next_id(&self) -> String {
        let c = self.id_counter.get().wrapping_add(1);
        self.id_counter.set(c);
        format!("{}-{}", c, Self::unix_time_ms() as u64)
    }

    fn unix_time() -> u32 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as u32
    }

    fn unix_time_ms() -> f64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as f64
    }
}
