//! NIP-04: Encrypted Direct Message implementation
//!
//! This module implements the NIP-04 encryption standard for Nostr,
//! which uses AES-256-CBC encryption with ECDH shared secrets.
//!
//! Note: NIP-04 is deprecated in favor of NIP-44 for new applications.

use aes::Aes256;
use base64::engine::{general_purpose, Engine};
use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use getrandom::getrandom;
use k256::{PublicKey as K256PublicKey, SecretKey as K256SecretKey};

use crate::signer::SignerError;
use crate::types::nostr::{PublicKey, SecretKey};

type Aes256CbcEnc = cbc::Encryptor<Aes256>;
type Aes256CbcDec = cbc::Decryptor<Aes256>;

/// NIP-04 specific errors
#[derive(Debug, thiserror::Error)]
pub enum Nip04Error {
    #[error("Invalid content format")]
    InvalidContentFormat,

    #[error("Base64 decode error")]
    Base64Decode,

    #[error("UTF-8 encoding error")]
    Utf8Encode,

    #[error("Wrong block mode")]
    WrongBlockMode,

    #[error("Invalid key: {0}")]
    InvalidKey(String),

    #[error("Encryption failed: {0}")]
    EncryptionFailed(String),

    #[error("Random generation failed")]
    RandomGenerationFailed,
}

impl From<Nip04Error> for SignerError {
    fn from(e: Nip04Error) -> Self {
        SignerError::CryptoError(e.to_string())
    }
}

/// Generate a shared secret key using ECDH
///
/// This implements the same shared key generation as the reference implementation,
/// using secp256k1 ECDH followed by SHA256 hashing.
fn generate_shared_key(
    secret_key: &SecretKey,
    public_key: &PublicKey,
) -> Result<[u8; 32], Nip04Error> {
    // Convert our types to k256 types
    let sk = K256SecretKey::from_bytes((&secret_key.0).into())
        .map_err(|e| Nip04Error::InvalidKey(format!("Invalid secret key: {}", e)))?;

    let pk_bytes = &public_key.0;
    // For k256, we need to add the compressed point prefix if not present
    let pk = if pk_bytes.len() == 32 {
        // It's an x-coordinate only, we need to construct the full public key
        // In Nostr, public keys are typically x-only (32 bytes)
        // We need to convert to a full public key for ECDH
        let mut full_pk = vec![0x02]; // Use compressed format with even y
        full_pk.extend_from_slice(pk_bytes);
        K256PublicKey::from_sec1_bytes(&full_pk)
            .or_else(|_| {
                // Try with odd y if even didn't work
                full_pk[0] = 0x03;
                K256PublicKey::from_sec1_bytes(&full_pk)
            })
            .map_err(|e| Nip04Error::InvalidKey(format!("Invalid public key: {}", e)))?
    } else {
        K256PublicKey::from_sec1_bytes(pk_bytes)
            .map_err(|e| Nip04Error::InvalidKey(format!("Invalid public key format: {}", e)))?
    };

    // Perform ECDH
    let shared_secret = k256::ecdh::diffie_hellman(sk.to_nonzero_scalar(), pk.as_affine());

    // IMPORTANT: NIP-04 uses the raw X coordinate of the shared point as the key,
    // WITHOUT hashing (unlike standard ECDH)
    // The shared_secret.raw_secret_bytes() gives us the X coordinate directly
    let mut key = [0u8; 32];
    key.copy_from_slice(shared_secret.raw_secret_bytes());

    // Return the raw X coordinate as the key (no SHA256 hashing!)
    Ok(key)
}

/// Generate secure random bytes using getrandom
fn generate_iv() -> Result<[u8; 16], Nip04Error> {
    let mut iv = [0u8; 16];
    getrandom(&mut iv).map_err(|_| Nip04Error::RandomGenerationFailed)?;
    Ok(iv)
}

/// Encrypt a message using NIP-04
///
/// This implements the standard NIP-04 encryption:
/// 1. Generate shared key via ECDH
/// 2. Generate random IV
/// 3. Encrypt with AES-256-CBC
/// 4. Encode as base64 with ?iv= parameter
pub fn encrypt(
    secret_key: &SecretKey,
    public_key: &PublicKey,
    content: &str,
) -> Result<String, Nip04Error> {
    // Generate shared key
    let key = generate_shared_key(secret_key, public_key)?;

    // Generate random IV using getrandom
    let iv = generate_iv()?;

    // Create cipher
    let cipher = Aes256CbcEnc::new(&key.into(), &iv.into());

    // Encrypt with PKCS7 padding
    let ciphertext = cipher.encrypt_padded_vec_mut::<Pkcs7>(content.as_bytes());

    // Encode result as base64
    Ok(format!(
        "{}?iv={}",
        general_purpose::STANDARD.encode(ciphertext),
        general_purpose::STANDARD.encode(iv)
    ))
}

/// Encrypt with a custom IV (useful for testing)
pub fn encrypt_with_iv(
    secret_key: &SecretKey,
    public_key: &PublicKey,
    content: &str,
    iv: [u8; 16],
) -> Result<String, Nip04Error> {
    // Generate shared key
    let key = generate_shared_key(secret_key, public_key)?;

    // Create cipher
    let cipher = Aes256CbcEnc::new(&key.into(), &iv.into());

    // Encrypt with PKCS7 padding
    let ciphertext = cipher.encrypt_padded_vec_mut::<Pkcs7>(content.as_bytes());

    // Encode result as base64
    Ok(format!(
        "{}?iv={}",
        general_purpose::STANDARD.encode(ciphertext),
        general_purpose::STANDARD.encode(iv)
    ))
}

/// Decrypt a message using NIP-04
///
/// This implements the standard NIP-04 decryption:
/// 1. Parse base64 content and IV
/// 2. Generate shared key via ECDH
/// 3. Decrypt with AES-256-CBC
/// 4. Return UTF-8 string
pub fn decrypt(
    secret_key: &SecretKey,
    public_key: &PublicKey,
    encrypted_content: &str,
) -> Result<String, Nip04Error> {
    let bytes = decrypt_to_bytes(secret_key, public_key, encrypted_content)?;
    String::from_utf8(bytes).map_err(|_| Nip04Error::Utf8Encode)
}

/// Decrypt to raw bytes (useful when content might not be UTF-8)
pub fn decrypt_to_bytes(
    secret_key: &SecretKey,
    public_key: &PublicKey,
    encrypted_content: &str,
) -> Result<Vec<u8>, Nip04Error> {
    // Parse the encrypted content format: "base64content?iv=base64iv"
    let parts: Vec<&str> = encrypted_content.split("?iv=").collect();
    if parts.len() != 2 {
        return Err(Nip04Error::InvalidContentFormat);
    }

    // Decode base64 content and IV
    let mut encrypted = general_purpose::STANDARD
        .decode(parts[0])
        .map_err(|_| Nip04Error::Base64Decode)?;
    let iv = general_purpose::STANDARD
        .decode(parts[1])
        .map_err(|_| Nip04Error::Base64Decode)?;

    // Generate shared key
    let key = generate_shared_key(secret_key, public_key)?;

    // Create cipher
    let cipher = Aes256CbcDec::new(&key.into(), iv.as_slice().into());

    // Decrypt with PKCS7 padding - note the mutable buffer
    let decrypted = cipher
        .decrypt_padded_vec_mut::<Pkcs7>(&mut encrypted)
        .map_err(|_| Nip04Error::WrongBlockMode)?;

    Ok(decrypted)
}
