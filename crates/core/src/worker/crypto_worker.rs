use crate::channel::{WorkerChannel, WorkerChannelSender};
#[cfg(feature = "crypto")]
use crate::crypto::signers::PrivateKeySigner;
#[cfg(all(feature = "crypto", target_arch = "wasm32"))]
use crate::crypto::signers::Nip07Signer;
#[cfg(feature = "crypto")]
use crate::crypto::signers::nip46::{Nip46Config, Nip46Signer};
use crate::generated::nostr::fb;
use crate::port::Port;
use crate::spawn::spawn_worker;
use crate::traits::Signer;
use crate::types::nostr::Template;
use futures::channel::mpsc;
use std::sync::Arc;
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// SharedPort: bridges a WorkerChannelSender to the Port trait so NIP-46
// transport can send frames into the connections worker.
// ---------------------------------------------------------------------------
struct SharedPort {
	sender: std::rc::Rc<std::cell::RefCell<Box<dyn WorkerChannelSender>>>,
}

impl Port for SharedPort {
	fn send(&self, bytes: &[u8]) -> Result<(), String> {
		self.sender
			.borrow()
			.send(bytes)
			.map_err(|e| e.to_string())
	}
}

// ---------------------------------------------------------------------------
// ActiveSigner: enum that holds whichever signer is currently active.
// Cloning the enum clones the *reference* (Rc/Arc) so the borrow on the
// RefCell can be released before an await point.
// ---------------------------------------------------------------------------
#[derive(Clone)]
enum ActiveSigner {
	Unset,
	DynSigner(Arc<dyn Signer>),
	#[cfg(feature = "crypto")]
	Pk(std::rc::Rc<PrivateKeySigner>),
	#[cfg(feature = "crypto")]
	Nip46(std::rc::Rc<Nip46Signer>),
	#[cfg(all(feature = "crypto", target_arch = "wasm32"))]
	Nip07(std::rc::Rc<Nip07Signer>),
}

impl ActiveSigner {
	async fn get_public_key(&self) -> Result<String, String> {
		match self {
			ActiveSigner::Unset => Err("no signer configured".to_string()),
			ActiveSigner::DynSigner(s) => s.get_public_key().await.map_err(|e| e.to_string()),
			#[cfg(feature = "crypto")]
			ActiveSigner::Pk(s) => s.get_public_key().map_err(|e| e.to_string()),
			#[cfg(feature = "crypto")]
			ActiveSigner::Nip46(s) => s.get_public_key().await.map_err(|e| e.to_string()),
			#[cfg(all(feature = "crypto", target_arch = "wasm32"))]
			ActiveSigner::Nip07(s) => s.get_public_key().await.map_err(|e| format!("{:?}", e)),
		}
	}

	async fn sign_event(&self, event_json: &str) -> Result<String, String> {
		match self {
			ActiveSigner::Unset => Err("no signer configured".to_string()),
			ActiveSigner::DynSigner(s) => s.sign_event(event_json).await.map_err(|e| e.to_string()),
			#[cfg(feature = "crypto")]
			ActiveSigner::Pk(s) => s.sign_event(event_json).await.map_err(|e| e.to_string()),
			#[cfg(feature = "crypto")]
			ActiveSigner::Nip46(s) => {
				let val = s.sign_event(event_json).await.map_err(|e| e.to_string())?;
				serde_json::to_string(&val).map_err(|e| e.to_string())
			}
			#[cfg(all(feature = "crypto", target_arch = "wasm32"))]
			ActiveSigner::Nip07(s) => {
				let signed = s.sign_event(event_json).await.map_err(|e| format!("{:?}", e))?;
				serde_json::to_string(&signed).map_err(|e| e.to_string())
			}
		}
	}

	async fn nip04_encrypt(&self, peer: &str, plaintext: &str) -> Result<String, String> {
		match self {
			ActiveSigner::Unset => Err("no signer configured".to_string()),
			ActiveSigner::DynSigner(s) => {
				s.nip04_encrypt(peer, plaintext).await.map_err(|e| e.to_string())
			}
			#[cfg(feature = "crypto")]
			ActiveSigner::Pk(s) => s.nip04_encrypt(peer, plaintext).map_err(|e| e.to_string()),
			#[cfg(feature = "crypto")]
			ActiveSigner::Nip46(s) => {
				s.nip04_encrypt(peer, plaintext).await.map_err(|e| e.to_string())
			}
			#[cfg(all(feature = "crypto", target_arch = "wasm32"))]
			ActiveSigner::Nip07(s) => {
				s.nip04_encrypt(peer, plaintext).await.map_err(|e| format!("{:?}", e))
			}
		}
	}

	async fn nip04_decrypt(&self, peer: &str, ciphertext: &str) -> Result<String, String> {
		match self {
			ActiveSigner::Unset => Err("no signer configured".to_string()),
			ActiveSigner::DynSigner(s) => {
				s.nip04_decrypt(peer, ciphertext).await.map_err(|e| e.to_string())
			}
			#[cfg(feature = "crypto")]
			ActiveSigner::Pk(s) => s.nip04_decrypt(peer, ciphertext).map_err(|e| e.to_string()),
			#[cfg(feature = "crypto")]
			ActiveSigner::Nip46(s) => {
				s.nip04_decrypt(peer, ciphertext).await.map_err(|e| e.to_string())
			}
			#[cfg(all(feature = "crypto", target_arch = "wasm32"))]
			ActiveSigner::Nip07(s) => {
				s.nip04_decrypt(peer, ciphertext).await.map_err(|e| format!("{:?}", e))
			}
		}
	}

	async fn nip44_encrypt(&self, peer: &str, plaintext: &str) -> Result<String, String> {
		match self {
			ActiveSigner::Unset => Err("no signer configured".to_string()),
			ActiveSigner::DynSigner(s) => {
				s.nip44_encrypt(peer, plaintext).await.map_err(|e| e.to_string())
			}
			#[cfg(feature = "crypto")]
			ActiveSigner::Pk(s) => s.nip44_encrypt(peer, plaintext).map_err(|e| e.to_string()),
			#[cfg(feature = "crypto")]
			ActiveSigner::Nip46(s) => {
				s.nip44_encrypt(peer, plaintext).await.map_err(|e| e.to_string())
			}
			#[cfg(all(feature = "crypto", target_arch = "wasm32"))]
			ActiveSigner::Nip07(s) => {
				s.nip44_encrypt(peer, plaintext).await.map_err(|e| format!("{:?}", e))
			}
		}
	}

	async fn nip44_decrypt(&self, peer: &str, ciphertext: &str) -> Result<String, String> {
		match self {
			ActiveSigner::Unset => Err("no signer configured".to_string()),
			ActiveSigner::DynSigner(s) => {
				s.nip44_decrypt(peer, ciphertext).await.map_err(|e| e.to_string())
			}
			#[cfg(feature = "crypto")]
			ActiveSigner::Pk(s) => s.nip44_decrypt(peer, ciphertext).map_err(|e| e.to_string()),
			#[cfg(feature = "crypto")]
			ActiveSigner::Nip46(s) => {
				s.nip44_decrypt(peer, ciphertext).await.map_err(|e| e.to_string())
			}
			#[cfg(all(feature = "crypto", target_arch = "wasm32"))]
			ActiveSigner::Nip07(s) => {
				s.nip44_decrypt(peer, ciphertext).await.map_err(|e| format!("{:?}", e))
			}
		}
	}

	async fn nip04_decrypt_between(
		&self,
		sender: &str,
		recipient: &str,
		ciphertext: &str,
	) -> Result<String, String> {
		match self {
			ActiveSigner::Unset => Err("no signer configured".to_string()),
			ActiveSigner::DynSigner(s) => s
				.nip04_decrypt_between(sender, recipient, ciphertext)
				.await
				.map_err(|e| e.to_string()),
			#[cfg(feature = "crypto")]
			ActiveSigner::Pk(s) => s
				.nip04_decrypt_between(sender, recipient, ciphertext)
				.map_err(|e| e.to_string()),
			#[cfg(feature = "crypto")]
			ActiveSigner::Nip46(s) => s
				.nip04_decrypt_between(sender, recipient, ciphertext)
				.await
				.map_err(|e| e.to_string()),
			#[cfg(all(feature = "crypto", target_arch = "wasm32"))]
			ActiveSigner::Nip07(s) => s
				.nip04_decrypt_between(sender, recipient, ciphertext)
				.await
				.map_err(|e| format!("{:?}", e)),
		}
	}

	async fn nip44_decrypt_between(
		&self,
		sender: &str,
		recipient: &str,
		ciphertext: &str,
	) -> Result<String, String> {
		match self {
			ActiveSigner::Unset => Err("no signer configured".to_string()),
			ActiveSigner::DynSigner(s) => s
				.nip44_decrypt_between(sender, recipient, ciphertext)
				.await
				.map_err(|e| e.to_string()),
			#[cfg(feature = "crypto")]
			ActiveSigner::Pk(s) => s
				.nip44_decrypt_between(sender, recipient, ciphertext)
				.map_err(|e| e.to_string()),
			#[cfg(feature = "crypto")]
			ActiveSigner::Nip46(s) => s
				.nip44_decrypt_between(sender, recipient, ciphertext)
				.await
				.map_err(|e| e.to_string()),
			#[cfg(all(feature = "crypto", target_arch = "wasm32"))]
			ActiveSigner::Nip07(s) => s
				.nip44_decrypt_between(sender, recipient, ciphertext)
				.await
				.map_err(|e| format!("{:?}", e)),
		}
	}
}

// ---------------------------------------------------------------------------
// URL parsing helpers (used by NIP-46 SetSigner)
// ---------------------------------------------------------------------------
#[cfg(feature = "crypto")]
#[derive(Debug)]
struct BunkerUrl {
	remote_pubkey: String,
	relays: Vec<String>,
	secret: Option<String>,
}

#[cfg(feature = "crypto")]
#[derive(Debug)]
struct NostrconnectUrl {
	client_pubkey: String,
	relays: Vec<String>,
	secret: String,
	app_name: Option<String>,
}

#[cfg(feature = "crypto")]
fn parse_bunker_url(url: &str) -> Result<BunkerUrl, String> {
	if !url.starts_with("bunker://") {
		return Err("Invalid bunker URL: must start with bunker://".to_string());
	}

	let url_part = &url[9..];
	let parts: Vec<&str> = url_part.split('?').collect();

	if parts.len() != 2 {
		return Err("Invalid bunker URL: missing query parameters".to_string());
	}

	let remote_pubkey = parts[0];
	if !remote_pubkey.chars().all(|c| c.is_ascii_hexdigit()) || remote_pubkey.len() != 64 {
		return Err("Invalid remote signer pubkey in bunker URL".to_string());
	}

	let query = parts[1];
	let mut relays = Vec::new();
	let mut secret = None;

	for pair in query.split('&') {
		let mut kv = pair.splitn(2, '=');
		let key = kv.next().unwrap_or("");
		let value = kv.next().unwrap_or("");
		let decoded = url::form_urlencoded::byte_serialize(value.as_bytes()).collect::<String>();
		// Actually form_urlencoded::byte_serialize encodes; we need decode.
		// Let's use percent_decode instead.
	}

	// Re-parse with proper decoding
	let params = url::Url::parse(&format!("http://localhost/?{}", query))
		.map_err(|e| format!("Invalid URL parameters: {}", e))?;

	for relay in params
		.query_pairs()
		.filter_map(|(k, v)| if k == "relay" { Some(v) } else { None })
	{
		relays.push(relay.to_string());
	}

	if relays.is_empty() {
		return Err("No relays specified in bunker URL".to_string());
	}

	secret = params.query_pairs().find_map(|(k, v)| {
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

#[cfg(feature = "crypto")]
fn parse_nostrconnect_url(url: &str) -> Result<NostrconnectUrl, String> {
	if !url.starts_with("nostrconnect://") {
		return Err("Invalid nostrconnect URL: must start with nostrconnect://".to_string());
	}

	let url_part = &url[15..];
	let parts: Vec<&str> = url_part.split('?').collect();

	if parts.len() != 2 {
		return Err("Invalid nostrconnect URL: missing query parameters".to_string());
	}

	let client_pubkey = parts[0];
	if !client_pubkey.chars().all(|c| c.is_ascii_hexdigit()) || client_pubkey.len() != 64 {
		return Err("Invalid client pubkey in nostrconnect URL".to_string());
	}

	let query = parts[1];
	let params = url::Url::parse(&format!("http://localhost/?{}", query))
		.map_err(|e| format!("Invalid URL parameters: {}", e))?;

	let mut relays = Vec::new();
	for relay in params
		.query_pairs()
		.filter_map(|(k, v)| if k == "relay" { Some(v) } else { None })
	{
		relays.push(relay.to_string());
	}

	if relays.is_empty() {
		return Err("No relays specified in nostrconnect URL".to_string());
	}

	let secret = params
		.query_pairs()
		.find_map(|(k, v)| if k == "secret" { Some(v.to_string()) } else { None })
		.ok_or_else(|| "Secret is required in nostrconnect URL".to_string())?;

	let app_name = params
		.query_pairs()
		.find_map(|(k, v)| if k == "name" { Some(v.to_string()) } else { None });

	Ok(NostrconnectUrl {
		client_pubkey: client_pubkey.to_string(),
		relays,
		secret,
		app_name,
	})
}

// ---------------------------------------------------------------------------
// CryptoWorker
// ---------------------------------------------------------------------------
pub struct CryptoWorker {
	active: std::rc::Rc<std::cell::RefCell<ActiveSigner>>,
	nip46_tx: std::cell::RefCell<Option<mpsc::Sender<Vec<u8>>>>,
}

impl CryptoWorker {
	pub fn new() -> Self {
		Self {
			active: std::rc::Rc::new(std::cell::RefCell::new(ActiveSigner::Unset)),
			nip46_tx: std::cell::RefCell::new(None),
		}
	}

	/// For backwards compatibility: inject an initial `Arc<dyn Signer>`
	/// (typically a `SwappableSigner`) so existing engine bootstrap keeps working.
	pub fn set_dyn_signer(&self, signer: Arc<dyn Signer>) {
		*self.active.borrow_mut() = ActiveSigner::DynSigner(signer);
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
		let to_connections_rc =
			std::rc::Rc::new(std::cell::RefCell::new(to_connections));

		// -------------------------------------------------------------------
		// Engine listener
		// -------------------------------------------------------------------
		let active_engine = self.active.clone();
		let to_main_engine = to_main;
		let nip46_tx_engine = self.nip46_tx.clone();
		let to_connections_rc_engine = to_connections_rc.clone();
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
								let _ = to_main_engine.send(&resp);
								continue;
							}
						};

						let result: Result<String, String> = match msg.content_type() {
							fb::MainContent::SignEvent => {
								if let Some(sign_event) = msg.content_as_sign_event() {
									let template =
										Template::from_flatbuffer(&sign_event.template());
									let event_json = template.to_json();
									let signer = active_engine.borrow().clone();
									signer.sign_event(&event_json).await
								} else {
									Err("missing sign_event content".to_string())
								}
							}
							fb::MainContent::GetPublicKey => {
								let signer = active_engine.borrow().clone();
								signer.get_public_key().await
							}
							fb::MainContent::SetSigner => {
								if let Some(set_signer) = msg.content_as_set_signer() {
									let set_signer_t = set_signer.unpack();
									match set_signer_t.signer_type {
										#[cfg(feature = "crypto")]
										fb::SignerTypeT::PrivateKey(pk) => {
											match PrivateKeySigner::new(&pk.private_key) {
												Ok(new_signer) => {
													let pubkey_hex = new_signer
														.get_public_key()
														.unwrap_or_default();
													*active_engine.borrow_mut() =
														ActiveSigner::Pk(
															std::rc::Rc::new(new_signer),
														);
													Ok(pubkey_hex)
												}
												Err(e) => Err(format!(
													"invalid private key: {}",
													e
												)),
											}
										}
										#[cfg(feature = "crypto")]
										fb::SignerTypeT::Nip46Bunker(bunker) => {
											match parse_bunker_url(&bunker.bunker_url)
												.map_err(|e| format!("parse bunker: {}", e))
											{
												Ok(parsed) => {
													let client_keys = bunker
														.client_secret
														.as_deref()
														.and_then(|s| crate::types::Keys::parse(s).ok());

													let cfg = Nip46Config {
														remote_signer_pubkey: parsed.remote_pubkey,
														relays: parsed.relays,
														use_nip44: true,
														app_name: None,
														expected_secret: parsed.secret,
													};

													let (tx, rx) = mpsc::channel::<Vec<u8>>(256);
													*nip46_tx_engine.borrow_mut() = Some(tx);

													let port = SharedPort {
														sender: to_connections_rc_engine.clone(),
													};
													let nip46 = std::rc::Rc::new(Nip46Signer::new(
														cfg,
														std::rc::Rc::new(std::cell::RefCell::new(
															port,
														)),
														rx,
														client_keys,
													));

													nip46.start(spawn_worker, None);
													*active_engine.borrow_mut() =
														ActiveSigner::Nip46(nip46.clone());

													let nip46_conn = nip46.clone();
													spawn_worker(async move {
														match nip46_conn.connect().await {
															Ok(_) => {
																match nip46_conn.get_public_key().await {
																	Ok(pk) => {
																		info!(
																			"[CryptoWorker] NIP-46 bunker connected, pubkey: {}",
																			&pk[..16.min(pk.len())]
																			);
																	}
																	Err(e) => {
																		warn!(
																			"[CryptoWorker] NIP-46 bunker get_public_key failed: {}",
																			e
																			);
																	}
																}
															}
															Err(e) => {
																warn!(
																	"[CryptoWorker] NIP-46 bunker connect failed: {}",
																	e
																);
															}
													}
													});

													Ok("NIP-46 bunker signer initialized".to_string())
												}
												Err(e) => Err(e),
											}
										}
										#[cfg(feature = "crypto")]
										fb::SignerTypeT::Nip46QR(qr) => {
											match parse_nostrconnect_url(&qr.nostrconnect_url)
												.map_err(|e| format!("parse nostrconnect: {}", e))
											{
												Ok(parsed) => {
													let client_keys = qr
														.client_secret
														.as_deref()
														.and_then(|s| crate::types::Keys::parse(s).ok());

													let cfg = Nip46Config {
														remote_signer_pubkey: String::new(),
														relays: parsed.relays,
														use_nip44: true,
														app_name: parsed.app_name,
														expected_secret: Some(parsed.secret),
													};

													let (tx, rx) = mpsc::channel::<Vec<u8>>(256);
													*nip46_tx_engine.borrow_mut() = Some(tx);

													let port = SharedPort {
														sender: to_connections_rc_engine.clone(),
													};
													let nip46 = std::rc::Rc::new(Nip46Signer::new(
														cfg,
														std::rc::Rc::new(std::cell::RefCell::new(
															port,
														)),
														rx,
														client_keys,
													));

													nip46.start(spawn_worker, None);
													*active_engine.borrow_mut() =
														ActiveSigner::Nip46(nip46.clone());

													Ok("NIP-46 QR signer initialized, awaiting discovery".to_string())
												}
												Err(e) => Err(e),
											}
										}
										#[cfg(all(feature = "crypto", target_arch = "wasm32"))]
									fb::SignerTypeT::Nip07(_) => {
										let nip07 = std::rc::Rc::new(Nip07Signer::new());
										*active_engine.borrow_mut() = ActiveSigner::Nip07(nip07.clone());
										nip07.get_public_key().await.map_err(|e| format!("{:?}", e))
									}
									_ => Err("unsupported signer type".to_string()),
									}
								} else {
									Err("missing set_signer content".to_string())
								}
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
						if let Err(e) = to_main_engine.send(&resp) {
							warn!("[CryptoWorker] failed to send response to main: {}", e);
						}
					}
					Err(_) => break,
				}
			}
			info!("[CryptoWorker] engine listener exiting");
		});

		// -------------------------------------------------------------------
		// Parser listener
		// -------------------------------------------------------------------
		let active_parser = self.active.clone();
		let to_parser_parser = to_parser;
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
								let _ = to_parser_parser.send(&resp);
								continue;
							}
						};

						let request_id = req.request_id();
						let payload = req.payload().unwrap_or("");
						let pubkey = req.pubkey().unwrap_or("");
						let signer = active_parser.borrow().clone();

						let result: Result<String, String> = match req.op() {
							fb::SignerOp::GetPubkey => signer.get_public_key().await,
							fb::SignerOp::SignEvent | fb::SignerOp::AuthEvent => {
								signer.sign_event(payload).await
							}
							fb::SignerOp::Nip04Encrypt => {
								signer.nip04_encrypt(pubkey, payload).await
							}
							fb::SignerOp::Nip04Decrypt => {
								signer.nip04_decrypt(pubkey, payload).await
							}
							fb::SignerOp::Nip44Encrypt => {
								signer.nip44_encrypt(pubkey, payload).await
							}
							fb::SignerOp::Nip44Decrypt => {
								signer.nip44_decrypt(pubkey, payload).await
							}
							fb::SignerOp::Nip04DecryptBetween => {
								let sender = req.sender_pubkey().unwrap_or("");
								let recipient = req.recipient_pubkey().unwrap_or("");
								if sender.is_empty() || recipient.is_empty() {
									Err("missing sender or recipient pubkey".to_string())
								} else {
									signer
										.nip04_decrypt_between(sender, recipient, payload)
										.await
								}
							}
							fb::SignerOp::Nip44DecryptBetween => {
								let sender = req.sender_pubkey().unwrap_or("");
								let recipient = req.recipient_pubkey().unwrap_or("");
								if sender.is_empty() || recipient.is_empty() {
									Err("missing sender or recipient pubkey".to_string())
								} else {
									signer
										.nip44_decrypt_between(sender, recipient, payload)
										.await
								}
							}
							fb::SignerOp::VerifyProof => Ok(String::new()),
							_ => Err(format!("unsupported SignerOp: {:?}", req.op())),
						};

						let resp = serialize_signer_response(request_id, result);
						if let Err(e) = to_parser_parser.send(&resp) {
							warn!("[CryptoWorker] failed to send response to parser: {}", e);
						}
					}
					Err(_) => break,
				}
			}
			info!("[CryptoWorker] parser listener exiting");
		});

		// -------------------------------------------------------------------
		// Connections listener
		// -------------------------------------------------------------------
		let active_connections = self.active.clone();
		let nip46_tx_connections = self.nip46_tx.clone();
		let to_connections_rc_connections = to_connections_rc.clone();
		spawn_worker(async move {
			info!("[CryptoWorker] connections listener started");
			loop {
				match from_connections.recv().await {
					Ok(bytes) => {
						let req = match flatbuffers::root::<fb::SignerRequest>(&bytes) {
							Ok(r) => r,
							Err(e) => {
								// Try NIP-46 pump first
								if let Some(tx) = nip46_tx_connections.borrow_mut().as_mut() {
									let _ = tx.try_send(bytes);
									continue;
								}
								warn!(
									"[CryptoWorker] failed to decode SignerRequest from connections: {}",
									e
								);
								let resp = serialize_signer_response(
									0,
									Err(format!("decode failed: {}", e)),
								);
								let _ = to_connections_rc_connections.borrow().send(&resp);
								continue;
							}
						};

						let request_id = req.request_id();
						let payload = req.payload().unwrap_or("");
						let signer = active_connections.borrow().clone();

						let result: Result<String, String> = match req.op() {
							fb::SignerOp::AuthEvent => {
								match signer.sign_event(payload).await {
									Ok(signed) => {
										if let Ok(parsed) =
											serde_json::from_str::<serde_json::Value>(payload)
										{
											let relay_url = parsed["relay"]
												.as_str()
												.unwrap_or("")
												.to_string();
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
						if let Err(e) = to_connections_rc_connections.borrow().send(&resp) {
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

				let worker = CryptoWorker::new();
				worker.set_dyn_signer(Arc::new(FailingJsonSigner));
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

				let worker = CryptoWorker::new();
				worker.set_dyn_signer(Arc::new(FailingDecryptSigner));
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

				let worker = CryptoWorker::new();
				worker.set_dyn_signer(Arc::new(UnavailableSigner));
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

				let worker = CryptoWorker::new();
				worker.set_dyn_signer(Arc::new(SimpleSigner));
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
