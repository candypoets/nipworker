use crate::traits::{Signer, SignerError};
use crate::types::nostr::Template;
use async_lock::RwLock;
use std::sync::Arc;

/// A Signer wrapper that allows hot-swapping the underlying signer at runtime.
/// Used by NostrEngine to support changing signers (e.g. private key → NIP-07)
/// without recreating the engine.
pub struct SwappableSigner {
	inner: RwLock<Arc<dyn Signer>>,
}

impl SwappableSigner {
	pub fn new(signer: Arc<dyn Signer>) -> Self {
		Self {
			inner: RwLock::new(signer),
		}
	}

	pub async fn set(&self, signer: Arc<dyn Signer>) {
		*self.inner.write().await = signer;
	}
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait::async_trait(?Send)]
impl Signer for SwappableSigner {
	async fn get_public_key(&self) -> Result<String, SignerError> {
		self.inner.read().await.get_public_key().await
	}

	async fn sign_event(&self, event_json: &str) -> Result<String, SignerError> {
		self.inner.read().await.sign_event(event_json).await
	}

	async fn nip04_encrypt(&self, peer: &str, plaintext: &str) -> Result<String, SignerError> {
		self.inner.read().await.nip04_encrypt(peer, plaintext).await
	}

	async fn nip04_decrypt(&self, peer: &str, ciphertext: &str) -> Result<String, SignerError> {
		self.inner.read().await.nip04_decrypt(peer, ciphertext).await
	}

	async fn nip44_encrypt(&self, peer: &str, plaintext: &str) -> Result<String, SignerError> {
		self.inner.read().await.nip44_encrypt(peer, plaintext).await
	}

	async fn nip44_decrypt(&self, peer: &str, ciphertext: &str) -> Result<String, SignerError> {
		self.inner.read().await.nip44_decrypt(peer, ciphertext).await
	}

	async fn nip04_decrypt_between(
		&self,
		sender: &str,
		recipient: &str,
		ciphertext: &str,
	) -> Result<String, SignerError> {
		self.inner.read().await.nip04_decrypt_between(sender, recipient, ciphertext).await
	}

	async fn nip44_decrypt_between(
		&self,
		sender: &str,
		recipient: &str,
		ciphertext: &str,
	) -> Result<String, SignerError> {
		self.inner.read().await.nip44_decrypt_between(sender, recipient, ciphertext).await
	}
}

#[cfg(target_arch = "wasm32")]
#[async_trait::async_trait(?Send)]
impl Signer for SwappableSigner {
	async fn get_public_key(&self) -> Result<String, SignerError> {
		self.inner.read().await.get_public_key().await
	}

	async fn sign_event(&self, event_json: &str) -> Result<String, SignerError> {
		self.inner.read().await.sign_event(event_json).await
	}

	async fn nip04_encrypt(&self, peer: &str, plaintext: &str) -> Result<String, SignerError> {
		self.inner.read().await.nip04_encrypt(peer, plaintext).await
	}

	async fn nip04_decrypt(&self, peer: &str, ciphertext: &str) -> Result<String, SignerError> {
		self.inner.read().await.nip04_decrypt(peer, ciphertext).await
	}

	async fn nip44_encrypt(&self, peer: &str, plaintext: &str) -> Result<String, SignerError> {
		self.inner.read().await.nip44_encrypt(peer, plaintext).await
	}

	async fn nip44_decrypt(&self, peer: &str, ciphertext: &str) -> Result<String, SignerError> {
		self.inner.read().await.nip44_decrypt(peer, ciphertext).await
	}

	async fn nip04_decrypt_between(
		&self,
		sender: &str,
		recipient: &str,
		ciphertext: &str,
	) -> Result<String, SignerError> {
		self.inner.read().await.nip04_decrypt_between(sender, recipient, ciphertext).await
	}

	async fn nip44_decrypt_between(
		&self,
		sender: &str,
		recipient: &str,
		ciphertext: &str,
	) -> Result<String, SignerError> {
		self.inner.read().await.nip44_decrypt_between(sender, recipient, ciphertext).await
	}
}
