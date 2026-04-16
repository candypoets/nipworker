use async_trait::async_trait;
use futures::channel::{mpsc, oneshot};
use nipworker_core::crypto::nostr_crypto::{compute_event_id, sign_event};
use nipworker_core::traits::{Signer, SignerError};
use nipworker_core::types::nostr::{Event, EventId, Keys, SecretKey, Template};
use std::cell::Cell;
use std::collections::HashMap;
use std::sync::Mutex;

/// WASM-compatible signer wrapping core cryptographic primitives.
/// NIP-07 and NIP-46 can be added later as additional variants.
pub struct LocalSigner {
	inner: Mutex<Option<Keys>>,
}

impl std::fmt::Debug for LocalSigner {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("LocalSigner")
			.field("configured", &self.inner.lock().unwrap().is_some())
			.finish()
	}
}

impl LocalSigner {
	pub fn new() -> Self {
		Self {
			inner: Mutex::new(None),
		}
	}

	pub fn set_private_key(&self, secret: &str) -> Result<(), SignerError> {
		let secret_key = SecretKey::from_hex(secret)
			.map_err(|e| SignerError::Other(format!("Invalid private key: {}", e)))?;
		let keys = Keys::new(secret_key);
		*self.inner.lock().unwrap() = Some(keys);
		Ok(())
	}
}

#[async_trait(?Send)]
impl Signer for LocalSigner {
	async fn get_public_key(&self) -> Result<String, SignerError> {
		let keys = {
			let inner = self.inner.lock().unwrap();
			inner.as_ref().cloned()
		};
		if let Some(keys) = keys {
			Ok(keys.public_key().to_hex())
		} else {
			Err(SignerError::Other("Signer not configured".to_string()))
		}
	}

	async fn sign_event(&self, event_json: &str) -> Result<String, SignerError> {
		let keys = {
			let inner = self.inner.lock().unwrap();
			inner.as_ref().cloned()
		};
		let keys = keys.ok_or_else(|| SignerError::Other("Signer not configured".to_string()))?;

		let template = Template::from_json(event_json)
			.map_err(|e| SignerError::Other(format!("Failed to parse template JSON: {}", e)))?;

		let mut event = Event {
			id: EventId([0u8; 32]),
			pubkey: keys.public_key(),
			created_at: template.created_at,
			kind: template.kind,
			tags: template.tags,
			content: template.content,
			sig: String::new(),
		};

		let event_id_hex = compute_event_id(
			&event.pubkey,
			event.created_at,
			event.kind,
			&event.tags,
			&event.content,
		);
		event.id = EventId::from_hex(&event_id_hex)
			.map_err(|e| SignerError::Other(format!("Failed to parse event ID: {}", e)))?;

		let secret_key = keys
			.secret_key()
			.map_err(|e| SignerError::Other(format!("Failed to get secret key: {}", e)))?;
		event.sig = sign_event(secret_key, &event.id)
			.map_err(|e| SignerError::Other(format!("Sign event failed: {}", e)))?;

		Ok(event.to_json())
	}

	async fn nip04_encrypt(&self, _peer: &str, _plaintext: &str) -> Result<String, SignerError> {
		Err(SignerError::Other(
			"NIP-04 encrypt not implemented in engine signer".to_string(),
		))
	}

	async fn nip04_decrypt(&self, _peer: &str, _ciphertext: &str) -> Result<String, SignerError> {
		Err(SignerError::Other(
			"NIP-04 decrypt not implemented in engine signer".to_string(),
		))
	}

	async fn nip44_encrypt(&self, _peer: &str, _plaintext: &str) -> Result<String, SignerError> {
		Err(SignerError::Other(
			"NIP-44 encrypt not implemented in engine signer".to_string(),
		))
	}

	async fn nip44_decrypt(&self, _peer: &str, _ciphertext: &str) -> Result<String, SignerError> {
		Err(SignerError::Other(
			"NIP-44 decrypt not implemented in engine signer".to_string(),
		))
	}
}

/// Proxy signer that forwards crypto operations to the main thread via MessagePort.
pub struct ProxySigner {
	request_tx: mpsc::UnboundedSender<(u64, String, serde_json::Value)>,
	pending: async_lock::Mutex<HashMap<u64, oneshot::Sender<Result<String, SignerError>>>>,
	next_id: Cell<u64>,
}

impl ProxySigner {
	pub fn new(
		request_tx: mpsc::UnboundedSender<(u64, String, serde_json::Value)>,
	) -> Self {
		Self {
			request_tx,
			pending: async_lock::Mutex::new(HashMap::new()),
			next_id: Cell::new(1),
		}
	}

	pub async fn handle_response(&self, id: u64, result: Result<String, String>) {
		if let Some(tx) = self.pending.lock().await.remove(&id) {
			let _ = tx.send(result.map_err(|e| SignerError::Other(e)));
		}
	}

	async fn call(&self, op: &str, payload: serde_json::Value) -> Result<String, SignerError> {
		let id = self.next_id.get();
		self.next_id.set(id + 1);

		let (tx, rx) = oneshot::channel();
		self.pending.lock().await.insert(id, tx);

		self.request_tx
			.unbounded_send((id, op.to_string(), payload))
			.map_err(|e| SignerError::Other(format!("Failed to send signer request: {}", e)))?;

		match rx.await {
			Ok(result) => result,
			Err(_) => Err(SignerError::Other("Signer request cancelled".to_string())),
		}
	}
}

#[async_trait(?Send)]
impl Signer for ProxySigner {
	async fn get_public_key(&self) -> Result<String, SignerError> {
		self.call("get_public_key", serde_json::json!({})).await
	}

	async fn sign_event(&self, event_json: &str) -> Result<String, SignerError> {
		self.call("sign_event", serde_json::json!({ "event_json": event_json }))
			.await
	}

	async fn nip04_encrypt(&self, peer: &str, plaintext: &str) -> Result<String, SignerError> {
		self.call(
			"nip04_encrypt",
			serde_json::json!({ "peer": peer, "plaintext": plaintext }),
		)
		.await
	}

	async fn nip04_decrypt(&self, peer: &str, ciphertext: &str) -> Result<String, SignerError> {
		self.call(
			"nip04_decrypt",
			serde_json::json!({ "peer": peer, "ciphertext": ciphertext }),
		)
		.await
	}

	async fn nip44_encrypt(&self, peer: &str, plaintext: &str) -> Result<String, SignerError> {
		self.call(
			"nip44_encrypt",
			serde_json::json!({ "peer": peer, "plaintext": plaintext }),
		)
		.await
	}

	async fn nip44_decrypt(&self, peer: &str, ciphertext: &str) -> Result<String, SignerError> {
		self.call(
			"nip44_decrypt",
			serde_json::json!({ "peer": peer, "ciphertext": ciphertext }),
		)
		.await
	}
}
