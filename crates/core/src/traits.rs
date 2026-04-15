use async_trait::async_trait;
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),
    #[error("Send failed: {0}")]
    SendFailed(String),
    #[error("Not connected")]
    NotConnected,
}

#[derive(Debug, Clone)]
pub enum TransportStatus {
    Connected(String),
    Failed(String),
    Closed(String),
}

#[async_trait(?Send)]
pub trait Transport: core::fmt::Debug {
    async fn connect(&self, url: &str) -> Result<(), TransportError>;
    fn disconnect(&self, url: &str);
    fn send(&self, url: &str, frame: String) -> Result<(), TransportError>;
    fn on_message(&self, url: &str, callback: Box<dyn Fn(String)>);
    fn on_status(&self, callback: Box<dyn Fn(TransportStatus)>);
}

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("DB error: {0}")]
    Db(String),
}

#[async_trait(?Send)]
pub trait Storage: core::fmt::Debug {
    async fn query(&self, filters: Vec<Value>) -> Result<Vec<Vec<u8>>, StorageError>;
    async fn persist(&self, event_bytes: &[u8]) -> Result<(), StorageError>;
}

#[derive(Debug, thiserror::Error)]
pub enum SignerError {
    #[error("Crypto error: {0}")]
    Crypto(String),
}

#[async_trait(?Send)]
pub trait Signer: core::fmt::Debug {
    async fn get_public_key(&self) -> Result<String, SignerError>;
    async fn sign_event(&self, event_json: &str) -> Result<String, SignerError>;
    async fn nip04_encrypt(&self, peer: &str, plaintext: &str) -> Result<String, SignerError>;
    async fn nip04_decrypt(&self, peer: &str, ciphertext: &str) -> Result<String, SignerError>;
    async fn nip44_encrypt(&self, peer: &str, plaintext: &str) -> Result<String, SignerError>;
    async fn nip44_decrypt(&self, peer: &str, ciphertext: &str) -> Result<String, SignerError>;
    async fn nip04_decrypt_between(&self, sender: &str, recipient: &str, ciphertext: &str) -> Result<String, SignerError>;
    async fn nip44_decrypt_between(&self, sender: &str, recipient: &str, ciphertext: &str) -> Result<String, SignerError>;
}
