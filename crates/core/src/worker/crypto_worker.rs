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
		to_main: Box<dyn WorkerChannelSender>,
		to_parser: Box<dyn WorkerChannelSender>,
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
							Ok(r) => format!("{{\"op\":\"{}\",\"result\":{}}}", op_name, r),
							Err(e) => format!(
								"{{\"op\":\"{}\",\"error\":\"{}\"}}",
								op_name, e
							),
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

		let signer_parser = self.signer;
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
