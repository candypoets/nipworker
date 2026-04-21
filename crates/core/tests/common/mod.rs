use async_trait::async_trait;
use nipworker_core::traits::{
	RelayTransport, Signer, Storage, TransportError, TransportStatus, StorageError, SignerError,
};
use nipworker_core::types::nostr::{Filter, Template};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};

// ============================================================================
// Mock Relay Transport
// ============================================================================

#[derive(Clone, Debug, PartialEq)]
pub enum TransportCall {
	Connect(String),
	Disconnect(String),
	Send(String, String),
}

pub struct MockRelayTransport {
	calls: Arc<Mutex<Vec<TransportCall>>>,
	message_callbacks: Arc<RwLock<HashMap<String, Box<dyn Fn(String)>>>>,
	status_callbacks: Arc<RwLock<HashMap<String, Box<dyn Fn(TransportStatus)>>>>,
	connect_result: Arc<RwLock<Result<(), TransportError>>>,
	send_fail_count: Arc<Mutex<usize>>,
	closed_urls: Arc<Mutex<HashSet<String>>>,
}

impl MockRelayTransport {
	pub fn new() -> Self {
		Self {
			calls: Arc::new(Mutex::new(Vec::new())),
			message_callbacks: Arc::new(RwLock::new(HashMap::new())),
			status_callbacks: Arc::new(RwLock::new(HashMap::new())),
			connect_result: Arc::new(RwLock::new(Ok(()))),
			send_fail_count: Arc::new(Mutex::new(0)),
			closed_urls: Arc::new(Mutex::new(HashSet::new())),
		}
	}

	pub fn set_connect_result(&self, result: Result<(), TransportError>) {
		*self.connect_result.write().unwrap() = result;
	}

	pub fn set_send_fail_count(&self, count: usize) {
		*self.send_fail_count.lock().unwrap() = count;
	}

	pub fn invoke_message_callback(&self, url: &str, msg: String) {
		let cbs = self.message_callbacks.read().unwrap();
		if let Some(cb) = cbs.get(url) {
			cb(msg);
		}
	}

	pub fn invoke_status_callback(&self, url: &str, status: TransportStatus) {
		if matches!(status, TransportStatus::Closed { .. }) {
			self.closed_urls.lock().unwrap().insert(url.to_string());
		}
		let cbs = self.status_callbacks.read().unwrap();
		if let Some(cb) = cbs.get(url) {
			cb(status);
		}
	}

	pub fn get_calls(&self) -> Vec<TransportCall> {
		self.calls.lock().unwrap().clone()
	}

	pub fn get_sent_frames(&self) -> Vec<(String, String)> {
		self.calls
			.lock()
			.unwrap()
			.iter()
			.filter_map(|c| match c {
				TransportCall::Send(url, frame) => Some((url.clone(), frame.clone())),
				_ => None,
			})
			.collect()
	}
}

#[async_trait(?Send)]
impl RelayTransport for MockRelayTransport {
	async fn connect(&self, url: &str) -> Result<(), TransportError> {
		self.calls
			.lock()
			.unwrap()
			.push(TransportCall::Connect(url.to_string()));
		let result = self.connect_result.read().unwrap().clone();
		if result.is_ok() {
			self.closed_urls.lock().unwrap().remove(url);
		}
		result
	}

	fn disconnect(&self, url: &str) {
		self.calls
			.lock()
			.unwrap()
			.push(TransportCall::Disconnect(url.to_string()));
	}

	async fn send(&self, url: &str, frame: String) -> Result<(), TransportError> {
		if self.closed_urls.lock().unwrap().contains(url) {
			return Err(TransportError::Other("connection closed".to_string()));
		}
		self.calls
			.lock()
			.unwrap()
			.push(TransportCall::Send(url.to_string(), frame.clone()));
		let mut remaining = self.send_fail_count.lock().unwrap();
		if *remaining > 0 {
			*remaining -= 1;
			Err(TransportError::Other("send failed".to_string()))
		} else {
			Ok(())
		}
	}

	fn on_message(&self, url: &str, callback: Box<dyn Fn(String)>) {
		self.message_callbacks
			.write()
			.unwrap()
			.insert(url.to_string(), callback);
	}

	fn on_status(&self, url: &str, callback: Box<dyn Fn(TransportStatus)>) {
		self.status_callbacks
			.write()
			.unwrap()
			.insert(url.to_string(), callback);
	}
}

// ============================================================================
// Mock Storage
// ============================================================================

pub struct MockStorage {
	query_results: Arc<Mutex<Vec<Vec<Vec<u8>>>>>,
	persisted: Arc<Mutex<Vec<Vec<u8>>>>,
	query_calls: Arc<Mutex<Vec<Vec<Filter>>>>,
}

impl MockStorage {
	pub fn new() -> Self {
		Self {
			query_results: Arc::new(Mutex::new(Vec::new())),
			persisted: Arc::new(Mutex::new(Vec::new())),
			query_calls: Arc::new(Mutex::new(Vec::new())),
		}
	}

	pub fn with_query_results(results: Vec<Vec<Vec<u8>>>) -> Self {
		let s = Self::new();
		*s.query_results.lock().unwrap() = results;
		s
	}

	pub fn get_persisted(&self) -> Vec<Vec<u8>> {
		self.persisted.lock().unwrap().clone()
	}

	pub fn get_query_calls(&self) -> Vec<Vec<Filter>> {
		self.query_calls.lock().unwrap().clone()
	}
}

#[async_trait(?Send)]
impl Storage for MockStorage {
	async fn query(&self, filters: Vec<Filter>) -> Result<Vec<Vec<u8>>, StorageError> {
		self.query_calls.lock().unwrap().push(filters);
		let mut results = self.query_results.lock().unwrap();
		if !results.is_empty() {
			Ok(results.remove(0))
		} else {
			Ok(Vec::new())
		}
	}

	async fn persist(&self, event_bytes: &[u8]) -> Result<(), StorageError> {
		self.persisted.lock().unwrap().push(event_bytes.to_vec());
		Ok(())
	}

	async fn initialize(&self) -> Result<(), StorageError> {
		Ok(())
	}
}

// ============================================================================
// Mock Signer
// ============================================================================

#[derive(Debug, Clone)]
pub enum SignerCall {
	GetPublicKey,
	SignEvent(String),
}

pub struct MockSigner {
	pub pubkey: String,
	pub signature: String,
	pub calls: Arc<Mutex<Vec<SignerCall>>>,
}

impl MockSigner {
	pub fn new(pubkey: &str, signature: &str) -> Self {
		Self {
			pubkey: pubkey.to_string(),
			signature: signature.to_string(),
			calls: Arc::new(Mutex::new(Vec::new())),
		}
	}
}

#[async_trait(?Send)]
impl Signer for MockSigner {
	async fn get_public_key(&self) -> Result<String, SignerError> {
		self.calls.lock().unwrap().push(SignerCall::GetPublicKey);
		Ok(self.pubkey.clone())
	}

	async fn sign_event(&self, event_json: &str) -> Result<String, SignerError> {
		self.calls
			.lock()
			.unwrap()
			.push(SignerCall::SignEvent(event_json.to_string()));

		let parsed: serde_json::Value = serde_json::from_str(event_json)
			.map_err(|e| SignerError::Other(format!("json parse: {}", e)))?;

		// Auth payload has "challenge" and "relay" fields
		if parsed.get("challenge").is_some() {
			let challenge = parsed["challenge"].as_str().unwrap_or("");
			let relay = parsed["relay"].as_str().unwrap_or("");
			let auth_event = serde_json::json!({
				"id": "0000000000000000000000000000000000000000000000000000000000000001",
				"pubkey": self.pubkey,
				"created_at": parsed["created_at"],
				"kind": 22242,
				"tags": [["challenge", challenge], ["relay", relay]],
				"content": "",
				"sig": self.signature
			});
			let result = serde_json::json!({
				"event": auth_event.to_string(),
				"relay": relay
			});
			return Ok(result.to_string());
		}

		// Regular event: ensure id, pubkey, sig are present
		let mut event = parsed;
		event["id"] = serde_json::Value::String(
			"0000000000000000000000000000000000000000000000000000000000000001".to_string(),
		);
		event["pubkey"] = serde_json::Value::String(self.pubkey.clone());
		event["sig"] = serde_json::Value::String(self.signature.clone());
		Ok(event.to_string())
	}

	async fn nip04_encrypt(&self, _peer: &str, _plaintext: &str) -> Result<String, SignerError> {
		Ok(String::new())
	}

	async fn nip04_decrypt(&self, _peer: &str, _ciphertext: &str) -> Result<String, SignerError> {
		Ok(String::new())
	}

	async fn nip44_encrypt(&self, _peer: &str, _plaintext: &str) -> Result<String, SignerError> {
		Ok(String::new())
	}

	async fn nip44_decrypt(&self, _peer: &str, _ciphertext: &str) -> Result<String, SignerError> {
		Ok(String::new())
	}

	async fn nip04_decrypt_between(
		&self,
		_sender: &str,
		_recipient: &str,
		_ciphertext: &str,
	) -> Result<String, SignerError> {
		Ok(String::new())
	}

	async fn nip44_decrypt_between(
		&self,
		_sender: &str,
		_recipient: &str,
		_ciphertext: &str,
	) -> Result<String, SignerError> {
		Ok(String::new())
	}
}

// ============================================================================
// Helpers
// ============================================================================

pub fn make_event_json(id: &str, pubkey: &str, kind: u16, content: &str, created_at: u64, sig: &str) -> String {
	serde_json::json!({
		"id": id,
		"pubkey": pubkey,
		"created_at": created_at,
		"kind": kind,
		"tags": [],
		"content": content,
		"sig": sig,
	})
	.to_string()
}

/// Build a WorkerMessage containing a NostrEvent (suitable for cache storage).
pub fn build_nostr_event_worker_message(
	sub_id: &str,
	url: &str,
	id: &str,
	pubkey: &str,
	kind: u16,
	content: &str,
	created_at: u64,
	sig: &str,
) -> Vec<u8> {
	use nipworker_core::generated::nostr::fb;
	use flatbuffers::FlatBufferBuilder;

	let mut builder = FlatBufferBuilder::new();
	let sid = builder.create_string(sub_id);
	let url_off = builder.create_string(url);
	let id_off = builder.create_string(id);
	let pubkey_off = builder.create_string(pubkey);
	let content_off = builder.create_string(content);
	let sig_off = builder.create_string(sig);

	let tags = builder.create_vector(&[] as &[flatbuffers::WIPOffset<fb::StringVec>]);
	let event = fb::NostrEvent::create(
		&mut builder,
		&fb::NostrEventArgs {
			id: Some(id_off),
			pubkey: Some(pubkey_off),
			kind,
			content: Some(content_off),
			tags: Some(tags),
			created_at: created_at as i32,
			sig: Some(sig_off),
		},
	);

	let wm = fb::WorkerMessage::create(
		&mut builder,
		&fb::WorkerMessageArgs {
			sub_id: Some(sid),
			url: Some(url_off),
			type_: fb::MessageType::NostrEvent,
			content_type: fb::Message::NostrEvent,
			content: Some(event.as_union_value()),
		},
	);
	builder.finish(wm, None);
	builder.finished_data().to_vec()
}
