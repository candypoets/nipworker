use crate::channel::{WorkerChannel, WorkerChannelSender};
use crate::generated::nostr::fb;
use crate::spawn::spawn_worker;
use crate::traits::{Signer, SignerError};
use futures::channel::oneshot;
use rustc_hash::FxHashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tracing::{info, warn};

/// Parser-facing client for the crypto worker.
/// Implements [`Signer`] by sending [`SignerRequest`] FlatBuffers over a
/// [`WorkerChannel`] and awaiting [`SignerResponse`] replies.
pub struct CryptoClient {
    sender: Box<dyn WorkerChannelSender>,
    pending: Arc<Mutex<FxHashMap<u64, oneshot::Sender<Result<String, String>>>>>,
    next_request_id: AtomicU64,
}

impl CryptoClient {
    pub fn new(channel: Box<dyn WorkerChannel>) -> Self {
        let sender = channel.clone_sender();
        let pending: Arc<Mutex<FxHashMap<u64, oneshot::Sender<Result<String, String>>>>> =
            Arc::new(Mutex::new(FxHashMap::default()));
        let pending_listener = pending.clone();

        spawn_worker(async move {
            let mut channel = channel;
            info!("[crypto-client] listener started");
            loop {
                match channel.recv().await {
                    Ok(bytes) => {
                        let resp = match flatbuffers::root::<fb::SignerResponse>(&bytes) {
                            Ok(r) => r,
                            Err(e) => {
                                warn!("[crypto-client] failed to decode SignerResponse: {}", e);
                                continue;
                            }
                        };
                        let req_id = resp.request_id();
                        let tx = {
                            let mut map = pending_listener.lock().unwrap();
                            map.remove(&req_id)
                        };
                        if let Some(tx) = tx {
                            let result = if let Some(err) = resp.error() {
                                Err(err.to_string())
                            } else {
                                Ok(resp.result().unwrap_or("").to_string())
                            };
                            if tx.send(result).is_err() {
                                warn!("[crypto-client] request {} caller dropped", req_id);
                            }
                        } else {
                            warn!(
                                "[crypto-client] unsolicited response for request {}",
                                req_id
                            );
                        }
                    }
                    Err(e) => {
                        warn!("[crypto-client] channel closed: {}", e);
                        break;
                    }
                }
            }
            info!("[crypto-client] listener exiting");
        });

        Self {
            sender,
            pending,
            next_request_id: AtomicU64::new(1),
        }
    }

    async fn call(
        &self,
        op: fb::SignerOp,
        payload: &str,
        pubkey: &str,
        sender: &str,
        recipient: &str,
    ) -> Result<String, SignerError> {
        let request_id = self.next_request_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        {
            let mut map = self.pending.lock().unwrap();
            map.insert(request_id, tx);
        }

        let mut builder = flatbuffers::FlatBufferBuilder::new();
        let payload_off = if payload.is_empty() {
            None
        } else {
            Some(builder.create_string(payload))
        };
        let pubkey_off = if pubkey.is_empty() {
            None
        } else {
            Some(builder.create_string(pubkey))
        };
        let sender_off = if sender.is_empty() {
            None
        } else {
            Some(builder.create_string(sender))
        };
        let recipient_off = if recipient.is_empty() {
            None
        } else {
            Some(builder.create_string(recipient))
        };

        let req = fb::SignerRequest::create(
            &mut builder,
            &fb::SignerRequestArgs {
                request_id,
                op,
                payload: payload_off,
                pubkey: pubkey_off,
                sender_pubkey: sender_off,
                recipient_pubkey: recipient_off,
            },
        );
        builder.finish(req, None);

        if let Err(e) = self.sender.send(builder.finished_data()) {
            let _ = self.pending.lock().unwrap().remove(&request_id);
            return Err(SignerError::Other(format!(
                "failed to send to crypto worker: {}",
                e
            )));
        }

        rx.await
            .map_err(|_| SignerError::Other("crypto worker response channel closed".to_string()))?
            .map_err(SignerError::Other)
    }
}

#[async_trait::async_trait(?Send)]
impl Signer for CryptoClient {
    async fn get_public_key(&self) -> Result<String, SignerError> {
        self.call(fb::SignerOp::GetPubkey, "", "", "", "").await
    }

    async fn sign_event(&self, event_json: &str) -> Result<String, SignerError> {
        self.call(fb::SignerOp::SignEvent, event_json, "", "", "")
            .await
    }

    async fn nip04_encrypt(&self, peer: &str, plaintext: &str) -> Result<String, SignerError> {
        self.call(fb::SignerOp::Nip04Encrypt, plaintext, peer, "", "")
            .await
    }

    async fn nip04_decrypt(&self, peer: &str, ciphertext: &str) -> Result<String, SignerError> {
        self.call(fb::SignerOp::Nip04Decrypt, ciphertext, peer, "", "")
            .await
    }

    async fn nip44_encrypt(&self, peer: &str, plaintext: &str) -> Result<String, SignerError> {
        self.call(fb::SignerOp::Nip44Encrypt, plaintext, peer, "", "")
            .await
    }

    async fn nip44_decrypt(&self, peer: &str, ciphertext: &str) -> Result<String, SignerError> {
        self.call(fb::SignerOp::Nip44Decrypt, ciphertext, peer, "", "")
            .await
    }

    async fn nip04_decrypt_between(
        &self,
        sender: &str,
        recipient: &str,
        ciphertext: &str,
    ) -> Result<String, SignerError> {
        self.call(
            fb::SignerOp::Nip04DecryptBetween,
            ciphertext,
            "",
            sender,
            recipient,
        )
        .await
    }

    async fn nip44_decrypt_between(
        &self,
        sender: &str,
        recipient: &str,
        ciphertext: &str,
    ) -> Result<String, SignerError> {
        self.call(
            fb::SignerOp::Nip44DecryptBetween,
            ciphertext,
            "",
            sender,
            recipient,
        )
        .await
    }
}
