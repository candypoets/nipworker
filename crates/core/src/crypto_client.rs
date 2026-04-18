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

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
	use super::*;
	use crate::channel::TokioWorkerChannel;
	use crate::traits::Signer;
	use std::future::Future;

	fn build_signer_response(request_id: u64, result: Option<&str>, error: Option<&str>) -> Vec<u8> {
		let mut builder = flatbuffers::FlatBufferBuilder::new();
		let result_off = result.map(|s| builder.create_string(s));
		let error_off = error.map(|s| builder.create_string(s));
		let resp = fb::SignerResponse::create(
			&mut builder,
			&fb::SignerResponseArgs {
				request_id,
				result: result_off,
				error: error_off,
			},
		);
		builder.finish(resp, None);
		builder.finished_data().to_vec()
	}

	fn noop_waker() -> std::task::Waker {
		use std::task::{RawWaker, RawWakerVTable, Waker};
		fn noop_clone(_: *const ()) -> RawWaker {
			noop_raw_waker()
		}
		fn noop(_: *const ()) {}
		fn noop_raw_waker() -> RawWaker {
			RawWaker::new(std::ptr::null(), &VTABLE)
		}
		static VTABLE: RawWakerVTable = RawWakerVTable::new(noop_clone, noop, noop, noop);
		unsafe { Waker::from_raw(noop_raw_waker()) }
	}

	#[tokio::test]
	async fn test_get_public_key_roundtrip() {
		let local = tokio::task::LocalSet::new();
		local
			.run_until(async {
				let (client_ch, mut server_ch) = TokioWorkerChannel::new_pair();
				let client = CryptoClient::new(Box::new(client_ch));

				let server = tokio::task::spawn_local(async move {
					let bytes = server_ch.recv().await.unwrap();
					let req = flatbuffers::root::<fb::SignerRequest>(&bytes).unwrap();
					assert_eq!(req.op(), fb::SignerOp::GetPubkey);
					let resp = build_signer_response(req.request_id(), Some("abc123"), None);
					server_ch.send(&resp).await.unwrap();
				});

				let result = client.get_public_key().await;
				assert_eq!(result.ok(), Some("abc123".to_string()));
				server.await.unwrap();
			})
			.await;
	}

	#[tokio::test]
	async fn test_sign_event_roundtrip() {
		let local = tokio::task::LocalSet::new();
		local
			.run_until(async {
				let (client_ch, mut server_ch) = TokioWorkerChannel::new_pair();
				let client = CryptoClient::new(Box::new(client_ch));

				let server = tokio::task::spawn_local(async move {
					let bytes = server_ch.recv().await.unwrap();
					let req = flatbuffers::root::<fb::SignerRequest>(&bytes).unwrap();
					assert_eq!(req.op(), fb::SignerOp::SignEvent);
					assert_eq!(req.payload(), Some("{\"kind\":1}"));
					let resp = build_signer_response(req.request_id(), Some("signed"), None);
					server_ch.send(&resp).await.unwrap();
				});

				let result = client.sign_event("{\"kind\":1}").await;
				assert_eq!(result.ok(), Some("signed".to_string()));
				server.await.unwrap();
			})
			.await;
	}

	#[tokio::test]
	async fn test_nip04_encrypt_roundtrip() {
		let local = tokio::task::LocalSet::new();
		local
			.run_until(async {
				let (client_ch, mut server_ch) = TokioWorkerChannel::new_pair();
				let client = CryptoClient::new(Box::new(client_ch));

				let server = tokio::task::spawn_local(async move {
					let bytes = server_ch.recv().await.unwrap();
					let req = flatbuffers::root::<fb::SignerRequest>(&bytes).unwrap();
					assert_eq!(req.op(), fb::SignerOp::Nip04Encrypt);
					assert_eq!(req.pubkey(), Some("pk"));
					assert_eq!(req.payload(), Some("pt"));
					let resp = build_signer_response(req.request_id(), Some("encrypted"), None);
					server_ch.send(&resp).await.unwrap();
				});

				let result = client.nip04_encrypt("pk", "pt").await;
				assert_eq!(result.ok(), Some("encrypted".to_string()));
				server.await.unwrap();
			})
			.await;
	}

	#[tokio::test]
	async fn test_nip44_decrypt_between_roundtrip() {
		let local = tokio::task::LocalSet::new();
		local
			.run_until(async {
				let (client_ch, mut server_ch) = TokioWorkerChannel::new_pair();
				let client = CryptoClient::new(Box::new(client_ch));

				let server = tokio::task::spawn_local(async move {
					let bytes = server_ch.recv().await.unwrap();
					let req = flatbuffers::root::<fb::SignerRequest>(&bytes).unwrap();
					assert_eq!(req.op(), fb::SignerOp::Nip44DecryptBetween);
					assert_eq!(req.sender_pubkey(), Some("sender"));
					assert_eq!(req.recipient_pubkey(), Some("recipient"));
					assert_eq!(req.payload(), Some("ct"));
					let resp = build_signer_response(req.request_id(), Some("decrypted"), None);
					server_ch.send(&resp).await.unwrap();
				});

				let result = client.nip44_decrypt_between("sender", "recipient", "ct").await;
				assert_eq!(result.ok(), Some("decrypted".to_string()));
				server.await.unwrap();
			})
			.await;
	}

	#[tokio::test]
	async fn test_error_response_mapped_to_signer_error() {
		let local = tokio::task::LocalSet::new();
		local
			.run_until(async {
				let (client_ch, mut server_ch) = TokioWorkerChannel::new_pair();
				let client = CryptoClient::new(Box::new(client_ch));

				let server = tokio::task::spawn_local(async move {
					let bytes = server_ch.recv().await.unwrap();
					let req = flatbuffers::root::<fb::SignerRequest>(&bytes).unwrap();
					let resp = build_signer_response(req.request_id(), None, Some("bad_key"));
					server_ch.send(&resp).await.unwrap();
				});

				let result = client.get_public_key().await;
				match result {
					Err(SignerError::Other(s)) => assert_eq!(s, "bad_key"),
					_ => panic!("expected SignerError::Other(\"bad_key\")"),
				}
				server.await.unwrap();
			})
			.await;
	}

	#[tokio::test]
	async fn test_unsolicited_response_is_logged_not_panics() {
		let local = tokio::task::LocalSet::new();
		local
			.run_until(async {
				let (client_ch, mut server_ch) = TokioWorkerChannel::new_pair();
				let client = CryptoClient::new(Box::new(client_ch));

				// Send an unsolicited response with an unknown request_id
				let unsolicited = build_signer_response(99999, Some("ignored"), None);
				server_ch.send(&unsolicited).await.unwrap();

				// Allow the listener task to process the unsolicited message
				tokio::task::yield_now().await;

				// Prove the client survived by doing another roundtrip
				let server = tokio::task::spawn_local(async move {
					let bytes = server_ch.recv().await.unwrap();
					let req = flatbuffers::root::<fb::SignerRequest>(&bytes).unwrap();
					assert_eq!(req.op(), fb::SignerOp::GetPubkey);
					let resp = build_signer_response(req.request_id(), Some("still_alive"), None);
					server_ch.send(&resp).await.unwrap();
				});

				let result = client.get_public_key().await;
				assert_eq!(result.ok(), Some("still_alive".to_string()));
				server.await.unwrap();
			})
			.await;
	}

	#[tokio::test]
	async fn test_caller_dropped_does_not_panic() {
		let local = tokio::task::LocalSet::new();
		local
			.run_until(async {
				let (client_ch, mut server_ch) = TokioWorkerChannel::new_pair();
				let client = CryptoClient::new(Box::new(client_ch));

				// Start a request but drop the future after polling it once
				{
					let mut fut = std::pin::pin!(client.get_public_key());
					let waker = noop_waker();
					let mut cx = std::task::Context::from_waker(&waker);
					let _ = fut.as_mut().poll(&mut cx);
				}

				// The request was sent; read it and inject the response anyway
				let bytes = server_ch.recv().await.unwrap();
				let req = flatbuffers::root::<fb::SignerRequest>(&bytes).unwrap();
				let resp = build_signer_response(req.request_id(), Some("orphaned"), None);
				server_ch.send(&resp).await.unwrap();

				// Allow the listener to process the response for the dropped caller
				tokio::task::yield_now().await;

				// Prove the background listener is still healthy
				let server = tokio::task::spawn_local(async move {
					let bytes = server_ch.recv().await.unwrap();
					let req = flatbuffers::root::<fb::SignerRequest>(&bytes).unwrap();
					assert_eq!(req.op(), fb::SignerOp::GetPubkey);
					let resp = build_signer_response(req.request_id(), Some("healthy"), None);
					server_ch.send(&resp).await.unwrap();
				});

				let result = client.get_public_key().await;
				assert_eq!(result.ok(), Some("healthy".to_string()));
				server.await.unwrap();
			})
			.await;
	}
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
