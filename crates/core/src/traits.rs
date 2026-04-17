use async_trait::async_trait;
use thiserror::Error;

#[derive(Error, Debug, Clone)]
pub enum TransportError {
    #[error("{0}")]
    Other(String),
}

#[derive(Error, Debug, Clone)]
pub enum StorageError {
    #[error("{0}")]
    Other(String),
}

#[derive(Error, Debug, Clone)]
pub enum SignerError {
    #[error("{0}")]
    Other(String),
}

#[derive(Debug, Clone)]
pub enum TransportStatus {
    Connected { url: String },
    Failed { url: String },
    Closed { url: String },
}

#[async_trait(?Send)]
pub trait RelayTransport {
    async fn connect(&self, url: &str) -> Result<(), TransportError>;
    fn disconnect(&self, url: &str);
    fn send(&self, url: &str, frame: String) -> Result<(), TransportError>;
    fn on_message(&self, url: &str, callback: Box<dyn Fn(String)>);
    fn on_status(&self, url: &str, callback: Box<dyn Fn(TransportStatus)>);
}

#[async_trait(?Send)]
pub trait Storage {
    async fn query(
        &self,
        filters: Vec<crate::types::nostr::Filter>,
    ) -> Result<Vec<Vec<u8>>, StorageError>;
    async fn persist(&self, event_bytes: &[u8]) -> Result<(), StorageError>;
    async fn initialize(&self) -> Result<(), StorageError>;
}

#[async_trait(?Send)]
pub trait Signer {
    async fn get_public_key(&self) -> Result<String, SignerError>;
    async fn sign_event(&self, event_json: &str) -> Result<String, SignerError>;
    async fn nip04_encrypt(&self, peer: &str, plaintext: &str) -> Result<String, SignerError>;
    async fn nip04_decrypt(&self, peer: &str, ciphertext: &str) -> Result<String, SignerError>;
    async fn nip44_encrypt(&self, peer: &str, plaintext: &str) -> Result<String, SignerError>;
    async fn nip44_decrypt(&self, peer: &str, ciphertext: &str) -> Result<String, SignerError>;
    async fn nip04_decrypt_between(
        &self,
        sender: &str,
        recipient: &str,
        ciphertext: &str,
    ) -> Result<String, SignerError>;
    async fn nip44_decrypt_between(
        &self,
        sender: &str,
        recipient: &str,
        ciphertext: &str,
    ) -> Result<String, SignerError>;
}
