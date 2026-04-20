use crate::channel::{WorkerChannel, WorkerChannelSender};
use crate::generated::nostr::fb;
use crate::spawn::spawn_worker;
use crate::traits::Signer;
use crate::types::nostr::Template;
use std::sync::Arc;
use tracing::{info, warn};

pub struct CryptoWorker {
	signer: Arc<dyn Signer>,
}

impl CryptoWorker {
	pub fn new(signer: Arc<dyn Signer>) -> Self {
		Self { signer }
	}

	pub fn run(
		self,
		mut from_engine: Box<dyn WorkerChannel>,
		mut from_parser: Box<dyn WorkerChannel>,
		mut from_connections: Box<dyn WorkerChannel>,
		to_main: Box<dyn WorkerChannelSender>,
		to_parser: Box<dyn WorkerChannelSender>,
		to_connections: Box<dyn WorkerChannelSender>,
	) {
		let signer_engine = self.signer.clone();
		spawn_worker(async move {
			info!("[CryptoWorker] engine listener started");
			loop {
				match from_engine.recv().await {
					Ok(bytes) => {
						let msg = match flatbuffers::root::<fb::MainMessage>(&bytes) {
							Ok(m) => m,
							Err(e) => {
								warn!("[CryptoWorker] failed to decode MainMessage: {}", e);
								let resp = serialize_raw_message(&format!(
									"{{\"op\":\"error\",\"error\":\"decode failed: {}\"}}",
									e
								));
								let _ = to_main.send(&resp);
								continue;
							}
						};

						let result: Result<String, String> = match msg.content_type() {
							fb::MainContent::SignEvent => {
								if let Some(sign_event) = msg.content_as_sign_event() {
									let template = Template::from_flatbuffer(&sign_event.template());
									let event_json = template.to_json();
									signer_engine
										.sign_event(&event_json)
										.await
										.map_err(|e| e.to_string())
								} else {
									Err("missing sign_event content".to_string())
								}
							}
							fb::MainContent::GetPublicKey => signer_engine
								.get_public_key()
								.await
								.map_err(|e| e.to_string()),
							fb::MainContent::SetSigner => {
								warn!("[CryptoWorker] SetSigner not yet supported");
								Err("not supported".to_string())
							}
							_ => {
								warn!(
									"[CryptoWorker] unexpected MainContent type from engine: {:?}",
									msg.content_type()
								);
								continue;
							}
						};

						let op_name = match msg.content_type() {
							fb::MainContent::SignEvent => "sign_event",
							fb::MainContent::GetPublicKey => "get_public_key",
							fb::MainContent::SetSigner => "set_signer",
							_ => "unknown",
						};
						let raw_json = match result {
							Ok(r) => {
								let val = serde_json::json!({"op": op_name, "result": r});
								val.to_string()
							}
							Err(e) => {
								let val = serde_json::json!({"op": op_name, "error": e});
								val.to_string()
							}
						};
						let resp = serialize_raw_message(&raw_json);
						if let Err(e) = to_main.send(&resp) {
							warn!("[CryptoWorker] failed to send response to main: {}", e);
						}
					}
					Err(_) => break,
				}
			}
			info!("[CryptoWorker] engine listener exiting");
		});

		let signer_parser = self.signer.clone();
		spawn_worker(async move {
			info!("[CryptoWorker] parser listener started");
			loop {
				match from_parser.recv().await {
					Ok(bytes) => {
						let req = match flatbuffers::root::<fb::SignerRequest>(&bytes) {
							Ok(r) => r,
							Err(e) => {
								warn!("[CryptoWorker] failed to decode SignerRequest: {}", e);
								let resp = serialize_signer_response(
									0,
									Err(format!("decode failed: {}", e)),
								);
								let _ = to_parser.send(&resp);
								continue;
							}
						};

						let request_id = req.request_id();
						let payload = req.payload().unwrap_or("");
						let pubkey = req.pubkey().unwrap_or("");

						let result: Result<String, String> = match req.op() {
							fb::SignerOp::GetPubkey => signer_parser
								.get_public_key()
								.await
								.map_err(|e| e.to_string()),
							fb::SignerOp::SignEvent | fb::SignerOp::AuthEvent => signer_parser
								.sign_event(payload)
								.await
								.map_err(|e| e.to_string()),
							fb::SignerOp::Nip04Encrypt => signer_parser
								.nip04_encrypt(pubkey, payload)
								.await
								.map_err(|e| e.to_string()),
							fb::SignerOp::Nip04Decrypt => signer_parser
								.nip04_decrypt(pubkey, payload)
								.await
								.map_err(|e| e.to_string()),
							fb::SignerOp::Nip44Encrypt => signer_parser
								.nip44_encrypt(pubkey, payload)
								.await
								.map_err(|e| e.to_string()),
							fb::SignerOp::Nip44Decrypt => signer_parser
								.nip44_decrypt(pubkey, payload)
								.await
								.map_err(|e| e.to_string()),
							fb::SignerOp::Nip04DecryptBetween => {
								let sender = req.sender_pubkey().unwrap_or("");
								let recipient = req.recipient_pubkey().unwrap_or("");
								if sender.is_empty() || recipient.is_empty() {
									Err("missing sender or recipient pubkey".to_string())
								} else {
									signer_parser
										.nip04_decrypt_between(sender, recipient, payload)
										.await
										.map_err(|e| e.to_string())
								}
							}
							fb::SignerOp::Nip44DecryptBetween => {
								let sender = req.sender_pubkey().unwrap_or("");
								let recipient = req.recipient_pubkey().unwrap_or("");
								if sender.is_empty() || recipient.is_empty() {
									Err("missing sender or recipient pubkey".to_string())
								} else {
									signer_parser
										.nip44_decrypt_between(sender, recipient, payload)
										.await
										.map_err(|e| e.to_string())
								}
							}
							fb::SignerOp::VerifyProof => Ok(String::new()),
							_ => Err(format!("unsupported SignerOp: {:?}", req.op())),
						};

						let resp = serialize_signer_response(request_id, result);
						if let Err(e) = to_parser.send(&resp) {
							warn!("[CryptoWorker] failed to send response to parser: {}", e);
						}
					}
					Err(_) => break,
				}
			}
			info!("[CryptoWorker] parser listener exiting");
		});

		let signer_connections = self.signer;
		spawn_worker(async move {
			info!("[CryptoWorker] connections listener started");
			loop {
				match from_connections.recv().await {
					Ok(bytes) => {
						let req = match flatbuffers::root::<fb::SignerRequest>(&bytes) {
							Ok(r) => r,
							Err(e) => {
								warn!(
									"[CryptoWorker] failed to decode SignerRequest from connections: {}",
									e
								);
								let resp = serialize_signer_response(
									0,
									Err(format!("decode failed: {}", e)),
								);
								let _ = to_connections.send(&resp);
								continue;
							}
						};

						let request_id = req.request_id();
						let payload = req.payload().unwrap_or("");

						let result: Result<String, String> = match req.op() {
							fb::SignerOp::AuthEvent => {
								match signer_connections.sign_event(payload).await {
									Ok(signed) => {
										if let Ok(parsed) =
											serde_json::from_str::<serde_json::Value>(payload)
										{
											let relay_url =
												parsed["relay"].as_str().unwrap_or("").to_string();
											Ok(serde_json::json!({"event": signed, "relay": relay_url})
												.to_string())
										} else {
											Ok(signed)
										}
									}
									Err(e) => Err(e.to_string()),
								}
							}
							_ => Err(format!(
								"unsupported SignerOp from connections: {:?}",
								req.op()
							)),
						};

						let resp = serialize_signer_response(request_id, result);
						if let Err(e) = to_connections.send(&resp) {
							warn!(
								"[CryptoWorker] failed to send response to connections: {}",
								e
							);
						}
					}
					Err(_) => break,
				}
			}
			info!("[CryptoWorker] connections listener exiting");
		});
	}
}

fn serialize_raw_message(raw_json: &str) -> Vec<u8> {
	let mut builder = flatbuffers::FlatBufferBuilder::new();
	let raw_str = builder.create_string(raw_json);
	let raw = fb::Raw::create(
		&mut builder,
		&fb::RawArgs { raw: Some(raw_str) },
	);
	let msg = fb::WorkerMessage::create(
		&mut builder,
		&fb::WorkerMessageArgs {
			sub_id: None,
			url: None,
			type_: fb::MessageType::Raw,
			content_type: fb::Message::Raw,
			content: Some(raw.as_union_value()),
		},
	);
	builder.finish(msg, None);
	builder.finished_data().to_vec()
}

fn serialize_signer_response(request_id: u64, result: Result<String, String>) -> Vec<u8> {
	let mut builder = flatbuffers::FlatBufferBuilder::new();
	let (result_off, err_off) = match result {
		Ok(s) => (Some(builder.create_string(&s)), None),
		Err(e) => (None, Some(builder.create_string(&e))),
	};
	let resp = fb::SignerResponse::create(
		&mut builder,
		&fb::SignerResponseArgs {
			request_id,
			result: result_off,
			error: err_off,
		},
	);
	builder.finish(resp, None);
	builder.finished_data().to_vec()
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod new_tests {
	use super::*;
	use crate::channel::TokioWorkerChannel;
	use crate::traits::{Signer, SignerError};
	use crate::types::nostr::Template;
	use async_trait::async_trait;
	use std::sync::Arc;

	fn build_sign_event_main_msg(template: &Template) -> Vec<u8> {
		let mut builder = flatbuffers::FlatBufferBuilder::new();
		let fb_template = template.build_flatbuffer(&mut builder);
		let sign_event = fb::SignEvent::create(&mut builder, &fb::SignEventArgs { template: Some(fb_template) });
		let main_msg = fb::MainMessage::create(
			&mut builder,
			&fb::MainMessageArgs {
				content_type: fb::MainContent::SignEvent,
				content: Some(sign_event.as_union_value()),
			},
		);
		builder.finish(main_msg, None);
		builder.finished_data().to_vec()
	}

	fn build_get_public_key_main_msg() -> Vec<u8> {
		let mut builder = flatbuffers::FlatBufferBuilder::new();
		let get_pk = fb::GetPublicKey::create(&mut builder, &fb::GetPublicKeyArgs {});
		let main_msg = fb::MainMessage::create(
			&mut builder,
			&fb::MainMessageArgs {
				content_type: fb::MainContent::GetPublicKey,
				content: Some(get_pk.as_union_value()),
			},
		);
		builder.finish(main_msg, None);
		builder.finished_data().to_vec()
	}

	fn build_signer_request(
		op: fb::SignerOp,
		request_id: u64,
		payload: &str,
		pubkey: &str,
		sender_pubkey: &str,
		recipient_pubkey: &str,
	) -> Vec<u8> {
		let mut builder = flatbuffers::FlatBufferBuilder::new();
		let payload_off = if payload.is_empty() { None } else { Some(builder.create_string(payload)) };
		let pubkey_off = if pubkey.is_empty() { None } else { Some(builder.create_string(pubkey)) };
		let sender_off = if sender_pubkey.is_empty() { None } else { Some(builder.create_string(sender_pubkey)) };
		let recipient_off = if recipient_pubkey.is_empty() { None } else { Some(builder.create_string(recipient_pubkey)) };
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
		builder.finished_data().to_vec()
	}

	#[tokio::test]
	async fn test_sign_event_invalid_json() {
		struct FailingJsonSigner;

		#[async_trait(?Send)]
		impl Signer for FailingJsonSigner {
			async fn get_public_key(&self) -> Result<String, SignerError> {
				Ok("pubkey".to_string())
			}

			async fn sign_event(&self, event_json: &str) -> Result<String, SignerError> {
				let _: serde_json::Value = serde_json::from_str(event_json)
					.map_err(|e| SignerError::Other(format!("JSON parse failed: {}", e)))?;
				Ok("signed".to_string())
			}

			async fn nip04_encrypt(&self, _peer: &str, _plaintext: &str) -> Result<String, SignerError> {
				Ok("encrypted".to_string())
			}

			async fn nip04_decrypt(&self, _peer: &str, _ciphertext: &str) -> Result<String, SignerError> {
				Ok("decrypted".to_string())
			}

			async fn nip44_encrypt(&self, _peer: &str, _plaintext: &str) -> Result<String, SignerError> {
				Ok("encrypted".to_string())
			}

			async fn nip44_decrypt(&self, _peer: &str, _ciphertext: &str) -> Result<String, SignerError> {
				Ok("decrypted".to_string())
			}

			async fn nip04_decrypt_between(
				&self,
				_sender: &str,
				_recipient: &str,
				_ciphertext: &str,
			) -> Result<String, SignerError> {
				Ok("decrypted".to_string())
			}

			async fn nip44_decrypt_between(
				&self,
				_sender: &str,
				_recipient: &str,
				_ciphertext: &str,
			) -> Result<String, SignerError> {
				Ok("decrypted".to_string())
			}
		}

		let local = tokio::task::LocalSet::new();
		local
			.run_until(async {
				let (worker_engine, mut test_engine) = TokioWorkerChannel::new_pair();
				let (worker_parser, _test_parser) = TokioWorkerChannel::new_pair();
				let (mut test_main, worker_main) = TokioWorkerChannel::new_pair();
				let (_test_parser_rx, worker_parser_tx) = TokioWorkerChannel::new_pair();
				let (worker_connections, _test_connections) = TokioWorkerChannel::new_pair();
				let (_test_connections_rx, worker_connections_tx) = TokioWorkerChannel::new_pair();

				let signer = Arc::new(FailingJsonSigner);
				let worker = CryptoWorker::new(signer);
				worker.run(
					Box::new(worker_engine),
					Box::new(worker_parser),
					Box::new(worker_connections),
					worker_main.clone_sender(),
					worker_parser_tx.clone_sender(),
					worker_connections_tx.clone_sender(),
				);

				let template = Template {
					kind: 1,
					content: "hello\x00world".to_string(),
					tags: vec![],
					created_at: 0,
				};
				let msg = build_sign_event_main_msg(&template);
				test_engine.send(&msg).await.unwrap();

				let resp_bytes = test_main.recv().await.unwrap();
				let msg = flatbuffers::root::<fb::WorkerMessage>(&resp_bytes).unwrap();
				assert_eq!(msg.type_(), fb::MessageType::Raw);
				let raw = msg.content_as_raw().unwrap();
				let json: serde_json::Value = serde_json::from_str(raw.raw()).unwrap();
				assert_eq!(json["op"], "sign_event");
				assert!(json["error"].as_str().is_some());
			})
			.await;
	}

	#[tokio::test]
	async fn test_nip04_decrypt_invalid_ciphertext() {
		struct FailingDecryptSigner;

		#[async_trait(?Send)]
		impl Signer for FailingDecryptSigner {
			async fn get_public_key(&self) -> Result<String, SignerError> {
				Ok("pubkey".to_string())
			}

			async fn sign_event(&self, _event_json: &str) -> Result<String, SignerError> {
				Ok("signed".to_string())
			}

			async fn nip04_encrypt(&self, _peer: &str, _plaintext: &str) -> Result<String, SignerError> {
				Ok("encrypted".to_string())
			}

			async fn nip04_decrypt(&self, _peer: &str, _ciphertext: &str) -> Result<String, SignerError> {
				Err(SignerError::Other("Invalid ciphertext format".to_string()))
			}

			async fn nip44_encrypt(&self, _peer: &str, _plaintext: &str) -> Result<String, SignerError> {
				Ok("encrypted".to_string())
			}

			async fn nip44_decrypt(&self, _peer: &str, _ciphertext: &str) -> Result<String, SignerError> {
				Ok("decrypted".to_string())
			}

			async fn nip04_decrypt_between(
				&self,
				_sender: &str,
				_recipient: &str,
				_ciphertext: &str,
			) -> Result<String, SignerError> {
				Ok("decrypted".to_string())
			}

			async fn nip44_decrypt_between(
				&self,
				_sender: &str,
				_recipient: &str,
				_ciphertext: &str,
			) -> Result<String, SignerError> {
				Ok("decrypted".to_string())
			}
		}

		let local = tokio::task::LocalSet::new();
		local
			.run_until(async {
				let (worker_engine, _test_engine) = TokioWorkerChannel::new_pair();
				let (worker_parser, mut test_parser) = TokioWorkerChannel::new_pair();
				let (_test_main, worker_main) = TokioWorkerChannel::new_pair();
				let (mut test_parser_rx, worker_parser_tx) = TokioWorkerChannel::new_pair();
				let (worker_connections, _test_connections) = TokioWorkerChannel::new_pair();
				let (_test_connections_rx, worker_connections_tx) = TokioWorkerChannel::new_pair();

				let signer = Arc::new(FailingDecryptSigner);
				let worker = CryptoWorker::new(signer);
				worker.run(
					Box::new(worker_engine),
					Box::new(worker_parser),
					Box::new(worker_connections),
					worker_main.clone_sender(),
					worker_parser_tx.clone_sender(),
					worker_connections_tx.clone_sender(),
				);

				let req = build_signer_request(fb::SignerOp::Nip04Decrypt, 42, "bad_ciphertext", "pk", "", "");
				test_parser.send(&req).await.unwrap();

				let resp_bytes = test_parser_rx.recv().await.unwrap();
				let resp = flatbuffers::root::<fb::SignerResponse>(&resp_bytes).unwrap();
				assert_eq!(resp.request_id(), 42);
				assert_eq!(resp.result(), None);
				assert!(resp.error().is_some());
				assert!(resp.error().unwrap().contains("Invalid ciphertext"));
			})
			.await;
	}

	#[tokio::test]
	async fn test_signer_unavailable_error() {
		struct UnavailableSigner;

		#[async_trait(?Send)]
		impl Signer for UnavailableSigner {
			async fn get_public_key(&self) -> Result<String, SignerError> {
				Err(SignerError::Other("Signer is offline".to_string()))
			}

			async fn sign_event(&self, _event_json: &str) -> Result<String, SignerError> {
				Err(SignerError::Other("Signer is offline".to_string()))
			}

			async fn nip04_encrypt(&self, _peer: &str, _plaintext: &str) -> Result<String, SignerError> {
				Err(SignerError::Other("Signer is offline".to_string()))
			}

			async fn nip04_decrypt(&self, _peer: &str, _ciphertext: &str) -> Result<String, SignerError> {
				Err(SignerError::Other("Signer is offline".to_string()))
			}

			async fn nip44_encrypt(&self, _peer: &str, _plaintext: &str) -> Result<String, SignerError> {
				Err(SignerError::Other("Signer is offline".to_string()))
			}

			async fn nip44_decrypt(&self, _peer: &str, _ciphertext: &str) -> Result<String, SignerError> {
				Err(SignerError::Other("Signer is offline".to_string()))
			}

			async fn nip04_decrypt_between(
				&self,
				_sender: &str,
				_recipient: &str,
				_ciphertext: &str,
			) -> Result<String, SignerError> {
				Err(SignerError::Other("Signer is offline".to_string()))
			}

			async fn nip44_decrypt_between(
				&self,
				_sender: &str,
				_recipient: &str,
				_ciphertext: &str,
			) -> Result<String, SignerError> {
				Err(SignerError::Other("Signer is offline".to_string()))
			}
		}

		let local = tokio::task::LocalSet::new();
		local
			.run_until(async {
				let (worker_engine, mut test_engine) = TokioWorkerChannel::new_pair();
				let (worker_parser, _test_parser) = TokioWorkerChannel::new_pair();
				let (mut test_main, worker_main) = TokioWorkerChannel::new_pair();
				let (_test_parser_rx, worker_parser_tx) = TokioWorkerChannel::new_pair();
				let (worker_connections, _test_connections) = TokioWorkerChannel::new_pair();
				let (_test_connections_rx, worker_connections_tx) = TokioWorkerChannel::new_pair();

				let signer = Arc::new(UnavailableSigner);
				let worker = CryptoWorker::new(signer);
				worker.run(
					Box::new(worker_engine),
					Box::new(worker_parser),
					Box::new(worker_connections),
					worker_main.clone_sender(),
					worker_parser_tx.clone_sender(),
					worker_connections_tx.clone_sender(),
				);

				let msg = build_get_public_key_main_msg();
				test_engine.send(&msg).await.unwrap();

				let resp_bytes = test_main.recv().await.unwrap();
				let msg = flatbuffers::root::<fb::WorkerMessage>(&resp_bytes).unwrap();
				assert_eq!(msg.type_(), fb::MessageType::Raw);
				let raw = msg.content_as_raw().unwrap();
				let json: serde_json::Value = serde_json::from_str(raw.raw()).unwrap();
				assert_eq!(json["op"], "get_public_key");
				assert!(json["error"].as_str().is_some());
				assert!(json["error"].as_str().unwrap().contains("offline"));
			})
			.await;
	}

	#[tokio::test]
	async fn test_concurrent_crypto_requests() {
		struct SimpleSigner;

		#[async_trait(?Send)]
		impl Signer for SimpleSigner {
			async fn get_public_key(&self) -> Result<String, SignerError> {
				Ok("pubkey".to_string())
			}

			async fn sign_event(&self, _event_json: &str) -> Result<String, SignerError> {
				Ok("signature".to_string())
			}

			async fn nip04_encrypt(&self, _peer: &str, _plaintext: &str) -> Result<String, SignerError> {
				Ok("encrypted".to_string())
			}

			async fn nip04_decrypt(&self, _peer: &str, _ciphertext: &str) -> Result<String, SignerError> {
				Ok("decrypted".to_string())
			}

			async fn nip44_encrypt(&self, _peer: &str, _plaintext: &str) -> Result<String, SignerError> {
				Ok("encrypted".to_string())
			}

			async fn nip44_decrypt(&self, _peer: &str, _ciphertext: &str) -> Result<String, SignerError> {
				Ok("decrypted".to_string())
			}

			async fn nip04_decrypt_between(
				&self,
				_sender: &str,
				_recipient: &str,
				_ciphertext: &str,
			) -> Result<String, SignerError> {
				Ok("decrypted".to_string())
			}

			async fn nip44_decrypt_between(
				&self,
				_sender: &str,
				_recipient: &str,
				_ciphertext: &str,
			) -> Result<String, SignerError> {
				Ok("decrypted".to_string())
			}
		}

		let local = tokio::task::LocalSet::new();
		local
			.run_until(async {
				let (worker_engine, _test_engine) = TokioWorkerChannel::new_pair();
				let (worker_parser, mut test_parser) = TokioWorkerChannel::new_pair();
				let (_test_main, worker_main) = TokioWorkerChannel::new_pair();
				let (mut test_parser_rx, worker_parser_tx) = TokioWorkerChannel::new_pair();
				let (worker_connections, _test_connections) = TokioWorkerChannel::new_pair();
				let (_test_connections_rx, worker_connections_tx) = TokioWorkerChannel::new_pair();

				let signer = Arc::new(SimpleSigner);
				let worker = CryptoWorker::new(signer);
				worker.run(
					Box::new(worker_engine),
					Box::new(worker_parser),
					Box::new(worker_connections),
					worker_main.clone_sender(),
					worker_parser_tx.clone_sender(),
					worker_connections_tx.clone_sender(),
				);

				let mut sent_ids = Vec::new();
				for i in 0..100u64 {
					let req = build_signer_request(fb::SignerOp::SignEvent, i, "payload", "", "", "");
					test_parser.send(&req).await.unwrap();
					sent_ids.push(i);
				}

				let mut success_count = 0;
				let mut received_ids = Vec::new();
				for _ in 0..100 {
					let resp_bytes = test_parser_rx.recv().await.unwrap();
					let resp = flatbuffers::root::<fb::SignerResponse>(&resp_bytes).unwrap();
					assert_eq!(resp.result(), Some("signature"));
					received_ids.push(resp.request_id());
					success_count += 1;
				}
				assert_eq!(success_count, 100);

				received_ids.sort();
				assert_eq!(sent_ids, received_ids);
			})
			.await;
	}
}
