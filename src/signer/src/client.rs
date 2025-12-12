use gloo_timers::future::TimeoutFuture;
use js_sys::SharedArrayBuffer;
use shared::SabRing;
use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use tracing::{error, info, warn};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

/// Parser-facing client for the signer service SABs.
///
/// This is a minimal client that:
/// - writes requests into `signer_service_request`
/// - reads responses from `signer_service_response`
///
/// Encoding is a placeholder JSON envelope until the FlatBuffers schema is finalized:
/// { "request_id": u64, "op": "<string>", "payload": <json> }
///
/// The service currently echoes back payloads to prove the pipe. Once the full
/// FlatBuffers path is wired, this client can be updated to encode/decode FB
/// while keeping the same async API for parser users.
pub struct SignerClient {
    req: Rc<RefCell<SabRing>>,
    resp: Rc<RefCell<SabRing>>,
    pending: Rc<RefCell<HashMap<u64, futures_channel::oneshot::Sender<serde_json::Value>>>>,
    next_id: Rc<Cell<u64>>,
    pump_started: Rc<Cell<bool>>,
}

impl SignerClient {
    /// Construct a client from two SABs:
    /// - signer_service_request (writer)
    /// - signer_service_response (reader)
    pub fn new(
        signer_service_request: SharedArrayBuffer,
        signer_service_response: SharedArrayBuffer,
    ) -> Result<Self, JsValue> {
        let req = Rc::new(RefCell::new(SabRing::new(signer_service_request)?));
        let resp = Rc::new(RefCell::new(SabRing::new(signer_service_response)?));

        let client = Self {
            req,
            resp,
            pending: Rc::new(RefCell::new(HashMap::new())),
            next_id: Rc::new(Cell::new(1)),
            pump_started: Rc::new(Cell::new(false)),
        };

        client.ensure_pump();
        info!("[signer-client] initialized");
        Ok(client)
    }

    /// Ensure the single background pump is running to drain responses and
    /// deliver them to awaiting request futures.
    fn ensure_pump(&self) {
        if self.pump_started.get() {
            return;
        }
        self.pump_started.set(true);

        let resp = self.resp.clone();
        let pending = self.pending.clone();

        spawn_local(async move {
            let mut sleep_ms: u32 = 8;
            let max_sleep: u32 = 256;

            loop {
                let maybe = { resp.borrow_mut().read_next() };
                if let Some(bytes) = maybe {
                    sleep_ms = 8;

                    // Placeholder decode: JSON envelope
                    match serde_json::from_slice::<serde_json::Value>(&bytes) {
                        Ok(v) => {
                            // Route by request_id, if present
                            if let Some(rid) = v.get("request_id").and_then(|x| x.as_u64()) {
                                if let Some(tx) = pending.borrow_mut().remove(&rid) {
                                    // Ignore send failures (caller dropped)
                                    let _ = tx.send(v);
                                } else {
                                    warn!("[signer-client] response for unknown request_id={rid}");
                                }
                            } else {
                                warn!("[signer-client] response lacks request_id; dropping");
                            }
                        }
                        Err(e) => {
                            warn!("[signer-client] failed to parse response JSON: {e}");
                        }
                    }
                    continue;
                }

                TimeoutFuture::new(sleep_ms).await;
                sleep_ms = (sleep_ms * 2).min(max_sleep);
            }
        });

        info!("[signer-client] response pump started");
    }

    /// Get a new request id (monotonic u64, wraps on overflow)
    fn next_request_id(&self) -> u64 {
        let id = self.next_id.get();
        self.next_id.set(id.wrapping_add(1));
        id
    }

    /// Core generic call using the placeholder JSON envelope.
    ///
    /// On success, returns the JSON response body. While the service echoes for now,
    /// callers should expect a structured response once the FlatBuffers path is wired.
    pub async fn call(
        &self,
        op: &str,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        // Create a channel and register pending
        let rid = self.next_request_id();
        let (tx, rx) = futures_channel::oneshot::channel::<serde_json::Value>();
        self.pending.borrow_mut().insert(rid, tx);

        // Build placeholder JSON envelope
        let req_obj = serde_json::json!({
            "request_id": rid,
            "op": op,
            "payload": payload
        });

        // Write to ring
        let buf = match serde_json::to_vec(&req_obj) {
            Ok(b) => b,
            Err(e) => {
                self.pending.borrow_mut().remove(&rid);
                return Err(format!("serialize request: {}", e));
            }
        };

        let ok = self.req.borrow_mut().write(&buf);
        if !ok {
            self.pending.borrow_mut().remove(&rid);
            return Err("signer_service_request ring full (write dropped)".to_string());
        }

        // Await response (no timeout here; the service loop applies backpressure)
        match rx.await {
            Ok(v) => Ok(v),
            Err(_) => Err("signer response channel canceled".to_string()),
        }
    }

    /// Convenience: request public key from signer.
    /// Note: currently returns the entire JSON response (echo), until FB path is used.
    pub async fn get_public_key(&self) -> Result<serde_json::Value, String> {
        self.call("get_pubkey", serde_json::Value::Null).await
    }

    /// Convenience: sign an event Template represented as JSON.
    /// The payload should be a JSON object with fields expected by your Template.
    pub async fn sign_event(
        &self,
        template: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        self.call("sign_event", template).await
    }

    /// Convenience: NIP-04 encrypt via signer.
    pub async fn nip04_encrypt(
        &self,
        recipient_pubkey_hex: &str,
        plaintext: &str,
    ) -> Result<serde_json::Value, String> {
        let payload = serde_json::json!({
            "to": recipient_pubkey_hex,
            "content": plaintext
        });
        self.call("nip04_encrypt", payload).await
    }

    /// Convenience: NIP-44 encrypt via signer.
    pub async fn nip44_encrypt(
        &self,
        recipient_pubkey_hex: &str,
        plaintext: &str,
    ) -> Result<serde_json::Value, String> {
        let payload = serde_json::json!({
            "to": recipient_pubkey_hex,
            "content": plaintext
        });
        self.call("nip44_encrypt", payload).await
    }

    /// Convenience: NIP-04 decrypt via signer.
    pub async fn nip04_decrypt(
        &self,
        sender_pubkey_hex: &str,
        ciphertext: &str,
    ) -> Result<serde_json::Value, String> {
        let payload = serde_json::json!({
            "from": sender_pubkey_hex,
            "content": ciphertext
        });
        self.call("nip04_decrypt", payload).await
    }

    /// Convenience: NIP-44 decrypt via signer.
    pub async fn nip44_decrypt(
        &self,
        sender_pubkey_hex: &str,
        ciphertext: &str,
    ) -> Result<serde_json::Value, String> {
        let payload = serde_json::json!({
            "from": sender_pubkey_hex,
            "content": ciphertext
        });
        self.call("nip44_decrypt", payload).await
    }
}
