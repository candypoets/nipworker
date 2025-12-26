use flatbuffers::FlatBufferBuilder;
use gloo_timers::future::TimeoutFuture;
use js_sys::SharedArrayBuffer;
use shared::generated::nostr::fb;
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
/// - writes requests into `crypto_service_request`
/// - reads responses from `crypto_service_response`
///
/// Encoding is a placeholder JSON envelope until the FlatBuffers schema is finalized:
/// { "request_id": u64, "op": "<string>", "payload": <json> }
///
/// The service currently echoes back payloads to prove the pipe. Once the full
/// FlatBuffers path is wired, this client can be updated to encode/decode FB
/// while keeping the same async API for parser users.
pub struct CryptoClient {
    req: Rc<RefCell<SabRing>>,
    resp: Rc<RefCell<SabRing>>,
    pending: Rc<RefCell<HashMap<u64, futures_channel::oneshot::Sender<Result<String, String>>>>>,
    next_id: Rc<Cell<u64>>,
    pump_started: Rc<Cell<bool>>,
}

impl CryptoClient {
    /// Construct a client from two SABs:
    /// - crypto_service_request (writer)
    /// - crypto_service_response (reader)
    pub fn new(
        crypto_service_request: SharedArrayBuffer,
        crypto_service_response: SharedArrayBuffer,
    ) -> Result<Self, JsValue> {
        let req = Rc::new(RefCell::new(SabRing::new(crypto_service_request)?));
        let resp = Rc::new(RefCell::new(SabRing::new(crypto_service_response)?));

        let client = Self {
            req,
            resp,
            pending: Rc::new(RefCell::new(HashMap::new())),
            next_id: Rc::new(Cell::new(1)),
            pump_started: Rc::new(Cell::new(false)),
        };

        client.ensure_pump();
        info!("[crypto-client] initialized");
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

                    // Decode FlatBuffers SignerResponse and forward raw result/error to callers
                    match flatbuffers::root::<fb::SignerResponse>(&bytes) {
                        Ok(resp) => {
                            let rid = resp.request_id();
                            let result_str = resp.result().unwrap_or("");
                            let error_str = resp.error().unwrap_or("");

                            if let Some(tx) = pending.borrow_mut().remove(&rid) {
                                if !error_str.is_empty() {
                                    let _ = tx.send(Err(error_str.to_string()));
                                } else {
                                    let _ = tx.send(Ok(result_str.to_string()));
                                }
                            } else {
                                warn!("[crypto-client] response for unknown request_id={rid}");
                            }
                        }
                        Err(e) => {
                            warn!(
                                "[crypto-client] failed to decode SignerResponse FB: {:?}",
                                e
                            );
                        }
                    }
                    continue;
                }

                TimeoutFuture::new(sleep_ms).await;
                sleep_ms = (sleep_ms * 2).min(max_sleep);
            }
        });

        info!("[crypto-client] response pump started");
    }

    /// Get a new request id (monotonic u64, wraps on overflow)
    fn next_request_id(&self) -> u64 {
        let id = self.next_id.get();
        self.next_id.set(id.wrapping_add(1));
        id
    }

    /// Core generic call using raw string protocol.
    ///
    /// - payload: for sign_event pass the template JSON; for nip04/44 pass plaintext/ciphertext
    /// - pubkey: recipient for encrypt ops, sender for decrypt ops
    pub async fn call_raw(
        &self,
        op: &str,
        payload: Option<&str>,
        pubkey: Option<&str>,
        sender_pubkey: Option<&str>,
        recipient_pubkey: Option<&str>,
    ) -> Result<String, String> {
        // Create a channel and register pending
        let rid = self.next_request_id();
        let (tx, rx) = futures_channel::oneshot::channel::<Result<String, String>>();
        self.pending.borrow_mut().insert(rid, tx);

        // Build FlatBuffers SignerRequest
        let mut fbb = FlatBufferBuilder::new();

        let payload_off = payload.map(|s| fbb.create_string(s));
        let pubkey_off = pubkey.map(|s| fbb.create_string(s));
        let sender_off = sender_pubkey.map(|s| fbb.create_string(s));
        let recipient_off = recipient_pubkey.map(|s| fbb.create_string(s));
        let op_enum = match op {
            "get_pubkey" => fb::SignerOp::GetPubkey,
            "sign_event" => fb::SignerOp::SignEvent,
            "nip04_encrypt" => fb::SignerOp::Nip04Encrypt,
            "nip04_decrypt" => fb::SignerOp::Nip04Decrypt,
            "nip44_encrypt" => fb::SignerOp::Nip44Encrypt,
            "nip44_decrypt" => fb::SignerOp::Nip44Decrypt,
            "nip04_decrypt_between" => fb::SignerOp::Nip04DecryptBetween,
            "nip44_decrypt_between" => fb::SignerOp::Nip44DecryptBetween,
            "verify_proof" => fb::SignerOp::VerifyProof,
            _ => fb::SignerOp::GetPubkey,
        };

        let req = fb::SignerRequest::create(
            &mut fbb,
            &fb::SignerRequestArgs {
                request_id: rid,
                op: op_enum,
                payload: payload_off,
                pubkey: pubkey_off,
                sender_pubkey: sender_off,
                recipient_pubkey: recipient_off,
            },
        );
        fbb.finish(req, None);
        let out = fbb.finished_data();

        // Write to ring
        let ok = self.req.borrow_mut().write(out);
        if !ok {
            self.pending.borrow_mut().remove(&rid);
            return Err("crypto_service_request ring full (write dropped)".to_string());
        }

        // Await response (no timeout here; the service loop applies backpressure)
        match rx.await {
            Ok(res) => res,
            Err(_) => Err("crypto response channel canceled".to_string()),
        }
    }

    /// Convenience: request public key from signer.
    /// Note: currently returns the entire JSON response (echo), until FB path is used.
    pub async fn get_public_key(&self) -> Result<String, String> {
        self.call_raw("get_pubkey", None, None, None, None).await
    }

    /// Convenience: sign an event Template represented as JSON.
    /// The payload should be a JSON object with fields expected by your Template.
    pub async fn sign_event(&self, template: String) -> Result<String, String> {
        self.call_raw("sign_event", Some(&template), None, None, None)
            .await
    }

    /// Convenience: NIP-04 encrypt via signer.
    pub async fn nip04_encrypt(
        &self,
        recipient_pubkey_hex: &str,
        plaintext: &str,
    ) -> Result<String, String> {
        self.call_raw(
            "nip04_encrypt",
            Some(plaintext),
            Some(recipient_pubkey_hex),
            None,
            None,
        )
        .await
    }

    /// Convenience: NIP-44 encrypt via signer.
    pub async fn nip44_encrypt(
        &self,
        recipient_pubkey_hex: &str,
        plaintext: &str,
    ) -> Result<String, String> {
        self.call_raw(
            "nip44_encrypt",
            Some(plaintext),
            Some(recipient_pubkey_hex),
            None,
            None,
        )
        .await
    }

    /// Convenience: NIP-04 decrypt via signer.
    pub async fn nip04_decrypt(
        &self,
        sender_pubkey_hex: &str,
        ciphertext: &str,
    ) -> Result<String, String> {
        self.call_raw(
            "nip04_decrypt",
            Some(ciphertext),
            Some(sender_pubkey_hex),
            None,
            None,
        )
        .await
    }

    /// Convenience: NIP-44 decrypt via signer.
    pub async fn nip44_decrypt(
        &self,
        sender_pubkey_hex: &str,
        ciphertext: &str,
    ) -> Result<String, String> {
        self.call_raw(
            "nip44_decrypt",
            Some(ciphertext),
            Some(sender_pubkey_hex),
            None,
            None,
        )
        .await
    }

    /// Between-decrypt NIP-04 using explicit sender/recipient pubkeys.
    pub async fn nip04_decrypt_between(
        &self,
        sender_pubkey_hex: &str,
        recipient_pubkey_hex: &str,
        ciphertext: &str,
    ) -> Result<String, String> {
        info!(
            "[crypto-client] nip04_decrypt_between sender={} recipient={} ciphertext_len={}",
            sender_pubkey_hex,
            recipient_pubkey_hex,
            ciphertext.len()
        );
        self.call_raw(
            "nip04_decrypt_between",
            Some(ciphertext),
            None,
            Some(sender_pubkey_hex),
            Some(recipient_pubkey_hex),
        )
        .await
    }

    /// Between-decrypt NIP-44 using explicit sender/recipient pubkeys.
    pub async fn nip44_decrypt_between(
        &self,
        sender_pubkey_hex: &str,
        recipient_pubkey_hex: &str,
        ciphertext: &str,
    ) -> Result<String, String> {
        self.call_raw(
            "nip44_decrypt_between",
            Some(ciphertext),
            None,
            Some(sender_pubkey_hex),
            Some(recipient_pubkey_hex),
        )
        .await
    }

    /// Verify a Cashu proof with DLEQ signature and return Y point if valid
    ///
    /// Arguments:
    /// - proof_json: JSON string of the Proof object
    /// - mint_keys_json: JSON string of mint keys map {amount: key_hex, ...}
    ///
    /// Returns Result<String, String>:
    /// - Ok(Y_point_hex): Proof is valid, Y point computed
    /// - Ok(""): Proof is invalid (DLEQ verification failed)
    /// - Err(error): Error occurred during verification
    pub async fn verify_proof(
        &self,
        proof_json: String,
        mint_keys_json: String,
    ) -> Result<String, String> {
        // Use delimiter to separate proof and mint_keys
        let payload = format!("{}|||{}", proof_json, mint_keys_json);

        let result = self.call_raw("verify_proof", Some(&payload), None, None, None).await?;

        // Result is Y point hex string if valid, empty string if invalid
        Ok(result)
    }
}
