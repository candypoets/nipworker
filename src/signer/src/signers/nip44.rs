// Copyright (c) 2022-2023 Yuki Kishimoto
// Copyright (c) 2023-2025 Rust Nostr Developers
// Distributed under the MIT software license
// Adapted for project dependencies while maintaining NIP44 v2 compliance

//! NIP44 (v2) - Encrypted Payloads
//!
//! <https://github.com/nostr-protocol/nips/blob/master/44.md>

use std::fmt;
use std::ops::Range;
use std::string::FromUtf8Error;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chacha20::cipher::{KeyIvInit, StreamCipher};
use chacha20::ChaCha20;
use getrandom::getrandom;
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use shared::types::{PublicKey, SecretKey};

use super::SignerError;

// Constants as per NIP44 specification
const VERSION: u8 = 2;
const MESSAGE_KEYS_SIZE: usize = 76;
const CHACHA_KEY_SIZE: usize = 32;
const CHACHA_NONCE_SIZE: usize = 12;
const HMAC_KEY_SIZE: usize = 32;
const CHACHA_KEY_RANGE: Range<usize> = 0..CHACHA_KEY_SIZE;
const CHACHA_NONCE_RANGE: Range<usize> = CHACHA_KEY_SIZE..CHACHA_KEY_SIZE + CHACHA_NONCE_SIZE;
const HMAC_KEY_RANGE: Range<usize> = CHACHA_KEY_SIZE + CHACHA_NONCE_SIZE..MESSAGE_KEYS_SIZE;

const MIN_PLAINTEXT_SIZE: usize = 1;
const MAX_PLAINTEXT_SIZE: usize = 65535;

/// NIP44 v2 Error types (keeping ErrorV2 name for compatibility)
#[derive(Debug, PartialEq, Eq)]
pub enum ErrorV2 {
    /// UTF-8 encoding error
    Utf8Encode(FromUtf8Error),
    /// HKDF Length
    HkdfLength(usize),
    /// Try from slice
    TryFromSlice,
    /// Message is empty
    MessageEmpty,
    /// Message is too long
    MessageTooLong,
    /// Invalid HMAC
    InvalidHmac,
    /// Invalid padding
    InvalidPadding,
    /// Invalid payload
    InvalidPayload,
    /// Unknown version
    UnknownVersion(u8),
    /// Decoding error
    DecodingError(String),
}

impl std::error::Error for ErrorV2 {}

impl fmt::Display for ErrorV2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Utf8Encode(e) => write!(f, "error while encoding to UTF-8: {e}"),
            Self::HkdfLength(size) => write!(f, "invalid Length for HKDF: {size}"),
            Self::TryFromSlice => f.write_str("could not convert slice to array"),
            Self::MessageEmpty => f.write_str("message empty"),
            Self::MessageTooLong => f.write_str("message too long"),
            Self::InvalidHmac => f.write_str("invalid HMAC"),
            Self::InvalidPadding => f.write_str("invalid padding"),
            Self::InvalidPayload => f.write_str("invalid payload"),
            Self::UnknownVersion(v) => write!(f, "unknown version: {}", v),
            Self::DecodingError(s) => write!(f, "decoding error: {}", s),
        }
    }
}

impl From<FromUtf8Error> for ErrorV2 {
    fn from(e: FromUtf8Error) -> Self {
        Self::Utf8Encode(e)
    }
}

impl From<ErrorV2> for SignerError {
    fn from(e: ErrorV2) -> Self {
        SignerError::CryptoError(e.to_string())
    }
}

/// Message keys derived from conversation key and nonce
struct MessageKeys {
    chacha_key: [u8; 32],
    chacha_nonce: [u8; 12],
    hmac_key: [u8; 32],
}

impl MessageKeys {
    /// Derive message keys from conversation key and nonce using HKDF-expand
    fn derive(conversation_key: &ConversationKey, nonce: &[u8; 32]) -> Result<Self, SignerError> {
        let hk = Hkdf::<Sha256>::from_prk(conversation_key.as_bytes())
            .map_err(|_| ErrorV2::HkdfLength(0))?;

        let mut okm = [0u8; MESSAGE_KEYS_SIZE];
        hk.expand(nonce, &mut okm)
            .map_err(|_| ErrorV2::HkdfLength(0))?;

        let mut chacha_key = [0u8; 32];
        let mut chacha_nonce = [0u8; 12];
        let mut hmac_key = [0u8; 32];

        chacha_key.copy_from_slice(&okm[CHACHA_KEY_RANGE]);
        chacha_nonce.copy_from_slice(&okm[CHACHA_NONCE_RANGE]);
        hmac_key.copy_from_slice(&okm[HMAC_KEY_RANGE]);

        Ok(MessageKeys {
            chacha_key,
            chacha_nonce,
            hmac_key,
        })
    }
}

/// NIP44 v2 Conversation Key
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConversationKey([u8; 32]);

impl fmt::Debug for ConversationKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Conversation key: <sensitive>")
    }
}

impl std::ops::Deref for ConversationKey {
    type Target = [u8; 32];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl ConversationKey {
    /// Construct conversation key from 32-byte array
    #[inline]
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Derive conversation key from secret key and public key
    #[inline]
    pub fn derive(secret_key: &SecretKey, public_key: &PublicKey) -> Result<Self, SignerError> {
        // Perform ECDH to get shared secret (x-coordinate only)
        let shared_x = ecdh_shared_secret(secret_key, public_key)?;

        // HKDF-extract with salt="nip44-v2" and IKM=shared_x
        // The conversation key is the PRK from HKDF-extract
        let (prk, _) = Hkdf::<Sha256>::extract(Some(b"nip44-v2"), &shared_x);

        // Convert the generic array to [u8; 32]
        let mut conversation_key = [0u8; 32];
        conversation_key.copy_from_slice(&prk);

        Ok(Self(conversation_key))
    }

    /// Compose Conversation Key from bytes
    #[inline]
    pub fn from_slice(slice: &[u8]) -> Result<Self, SignerError> {
        if slice.len() != 32 {
            return Err(SignerError::CryptoError(
                "Invalid conversation key length".to_string(),
            ));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(slice);
        Ok(Self(arr))
    }

    /// Get conversation key as bytes
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// Perform ECDH using k256 to get shared x-coordinate
fn ecdh_shared_secret(
    secret_key: &SecretKey,
    public_key: &PublicKey,
) -> Result<[u8; 32], SignerError> {
    use k256::{ecdh::diffie_hellman, PublicKey as K256PublicKey, SecretKey as K256SecretKey};

    // Convert secret key
    let k256_secret = K256SecretKey::from_slice(&secret_key.0)
        .map_err(|e| SignerError::CryptoError(format!("Invalid secret key: {}", e)))?;

    // Public key in Nostr is 32 bytes (x-coordinate only)
    // We need to recover the full point - try both possible y-coordinates
    let k256_public = {
        // Try with 0x02 prefix (even y)
        let mut compressed = vec![0x02];
        compressed.extend_from_slice(&public_key.0);
        K256PublicKey::from_sec1_bytes(&compressed)
            .or_else(|_| {
                // Try with 0x03 prefix (odd y)
                compressed[0] = 0x03;
                K256PublicKey::from_sec1_bytes(&compressed)
            })
            .map_err(|e| SignerError::CryptoError(format!("Invalid public key: {}", e)))?
    };

    // Perform ECDH - get shared secret
    let shared_secret = diffie_hellman(k256_secret.to_nonzero_scalar(), k256_public.as_affine());

    // Return raw x-coordinate (32 bytes) - unhashed as per NIP44 spec
    let mut result = [0u8; 32];
    result.copy_from_slice(shared_secret.raw_secret_bytes());
    Ok(result)
}

/// Calculate padded length according to NIP44 specification
fn calc_padded_len(unpadded_len: usize) -> usize {
    if unpadded_len <= 32 {
        return 32;
    }

    let next_power = 1 << (unpadded_len - 1).ilog2() + 1;
    let chunk = if next_power <= 256 {
        32
    } else {
        next_power / 8
    };

    chunk * ((unpadded_len - 1) / chunk + 1)
}

/// Pad plaintext according to NIP44 specification
fn pad(plaintext: &[u8]) -> Result<Vec<u8>, ErrorV2> {
    let len = plaintext.len();

    if len < MIN_PLAINTEXT_SIZE {
        return Err(ErrorV2::MessageEmpty);
    }

    if len > MAX_PLAINTEXT_SIZE {
        return Err(ErrorV2::MessageTooLong);
    }

    let padded_len = calc_padded_len(len);
    let mut padded = Vec::with_capacity(2 + padded_len);

    // Add length prefix (big-endian u16)
    padded.extend_from_slice(&(len as u16).to_be_bytes());
    // Add plaintext
    padded.extend_from_slice(plaintext);
    // Add zero padding
    padded.resize(2 + padded_len, 0);

    Ok(padded)
}

/// Remove padding from decrypted plaintext
fn unpad(padded: &[u8]) -> Result<Vec<u8>, ErrorV2> {
    if padded.len() < 2 {
        return Err(ErrorV2::InvalidPadding);
    }

    let unpadded_len = u16::from_be_bytes([padded[0], padded[1]]) as usize;

    if unpadded_len == 0 {
        return Err(ErrorV2::MessageEmpty);
    }

    if padded.len() < 2 + unpadded_len {
        return Err(ErrorV2::InvalidPadding);
    }

    let expected_len = 2 + calc_padded_len(unpadded_len);
    if padded.len() != expected_len {
        return Err(ErrorV2::InvalidPadding);
    }

    Ok(padded[2..2 + unpadded_len].to_vec())
}

/// Encrypt with NIP44 (v2)
///
/// **The result is NOT encoded in base64!**
#[inline]
pub fn encrypt_to_bytes(
    conversation_key: &ConversationKey,
    plaintext: &[u8],
) -> Result<Vec<u8>, SignerError> {
    // Generate random nonce
    let mut nonce = [0u8; 32];
    getrandom(&mut nonce)
        .map_err(|e| SignerError::CryptoError(format!("Failed to generate nonce: {}", e)))?;

    internal_encrypt_to_bytes_with_nonce(conversation_key, plaintext, Some(&nonce))
}

/// Internal encryption with specific nonce
fn internal_encrypt_to_bytes_with_nonce(
    conversation_key: &ConversationKey,
    plaintext: &[u8],
    override_random_nonce: Option<&[u8; 32]>,
) -> Result<Vec<u8>, SignerError> {
    // Generate or use provided nonce
    let nonce: [u8; 32] = match override_random_nonce {
        Some(nonce) => *nonce,
        None => {
            let mut nonce = [0u8; 32];
            getrandom(&mut nonce).map_err(|e| {
                SignerError::CryptoError(format!("Failed to generate nonce: {}", e))
            })?;
            nonce
        }
    };

    // Pad plaintext
    let padded = pad(plaintext)?;

    // Derive message keys
    let keys = MessageKeys::derive(conversation_key, &nonce)?;

    // Encrypt with ChaCha20
    let mut ciphertext = padded;
    let mut cipher = ChaCha20::new(&keys.chacha_key.into(), &keys.chacha_nonce.into());
    cipher.apply_keystream(&mut ciphertext);

    // Calculate HMAC over nonce || ciphertext
    let mut mac =
        Hmac::<Sha256>::new_from_slice(&keys.hmac_key).expect("HMAC can take any size key");
    mac.update(&nonce);
    mac.update(&ciphertext);
    let mac_bytes = mac.finalize().into_bytes();

    // Construct payload: version || nonce || ciphertext || mac
    let mut payload = Vec::with_capacity(1 + 32 + ciphertext.len() + 32);
    payload.push(VERSION);
    payload.extend_from_slice(&nonce);
    payload.extend_from_slice(&ciphertext);
    payload.extend_from_slice(&mac_bytes);

    Ok(payload)
}

/// Decrypt with NIP44 (v2)
///
/// **The payload MUST be already decoded from base64**
pub fn decrypt_to_bytes(
    conversation_key: &ConversationKey,
    payload: &[u8],
) -> Result<Vec<u8>, SignerError> {
    // Validate payload length
    let len = payload.len();
    if len < 99 || len > 65603 {
        return Err(ErrorV2::InvalidPayload.into());
    }

    // Parse payload
    let version = payload[0];
    if version != VERSION {
        return Err(ErrorV2::UnknownVersion(version).into());
    }

    let nonce = &payload[1..33];
    let ciphertext = &payload[33..len - 32];
    let mac = &payload[len - 32..];

    // Derive message keys
    let nonce_array: [u8; 32] = nonce.try_into().map_err(|_| ErrorV2::TryFromSlice)?;
    let keys = MessageKeys::derive(conversation_key, &nonce_array)?;

    // Verify HMAC
    let mut mac_verifier =
        Hmac::<Sha256>::new_from_slice(&keys.hmac_key).expect("HMAC can take any size key");
    mac_verifier.update(nonce);
    mac_verifier.update(ciphertext);

    // Constant-time comparison
    mac_verifier
        .verify_slice(mac)
        .map_err(|_| ErrorV2::InvalidHmac)?;

    // Decrypt with ChaCha20
    let mut plaintext_padded = ciphertext.to_vec();
    let mut cipher = ChaCha20::new(&keys.chacha_key.into(), &keys.chacha_nonce.into());
    cipher.apply_keystream(&mut plaintext_padded);

    // Remove padding
    let plaintext_bytes = unpad(&plaintext_padded)?;

    Ok(plaintext_bytes)
}

/// Helper function to encrypt string and return base64
pub fn encrypt(plaintext: &str, conversation_key: &ConversationKey) -> Result<String, SignerError> {
    let encrypted = encrypt_to_bytes(conversation_key, plaintext.as_bytes())?;
    Ok(BASE64.encode(encrypted))
}

/// Helper function to decrypt from base64 string
pub fn decrypt(payload: &str, conversation_key: &ConversationKey) -> Result<String, SignerError> {
    // Check for future-proof flag
    if payload.starts_with('#') {
        return Err(ErrorV2::UnknownVersion(0).into());
    }

    // Validate payload length (base64)
    let plen = payload.len();
    if plen < 132 || plen > 87472 {
        return Err(ErrorV2::InvalidPayload.into());
    }

    // Decode base64
    let data = BASE64
        .decode(payload)
        .map_err(|e| ErrorV2::DecodingError(e.to_string()))?;

    // Decrypt
    let plaintext_bytes = decrypt_to_bytes(conversation_key, &data)?;

    // Convert to string
    String::from_utf8(plaintext_bytes).map_err(|e| ErrorV2::Utf8Encode(e).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calc_padded_len() {
        assert_eq!(calc_padded_len(1), 32);
        assert_eq!(calc_padded_len(32), 32);
        assert_eq!(calc_padded_len(33), 64);
        assert_eq!(calc_padded_len(64), 64);
        assert_eq!(calc_padded_len(65), 96);
        assert_eq!(calc_padded_len(256), 256);
        assert_eq!(calc_padded_len(257), 320);
    }

    #[test]
    fn test_padding() {
        let plaintext = b"hello";
        let padded = pad(plaintext).unwrap();
        assert_eq!(padded.len(), 2 + 32); // 2 bytes length + 32 bytes padded content
        assert_eq!(padded[0..2], [0x00, 0x05]); // Length 5 in big-endian
        assert_eq!(&padded[2..7], b"hello");

        let unpadded = unpad(&padded).unwrap();
        assert_eq!(unpadded, plaintext);
    }

    #[test]
    fn test_log2_round_down() {
        fn log2_round_down(x: usize) -> u32 {
            if x == 0 {
                0
            } else {
                (usize::BITS - 1) - x.leading_zeros()
            }
        }

        assert_eq!(log2_round_down(0), 0);
        assert_eq!(log2_round_down(1), 0);
        assert_eq!(log2_round_down(2), 1);
        assert_eq!(log2_round_down(3), 1);
        assert_eq!(log2_round_down(4), 2);
        assert_eq!(log2_round_down(7), 2);
        assert_eq!(log2_round_down(8), 3);
        assert_eq!(log2_round_down(255), 7);
        assert_eq!(log2_round_down(256), 8);
    }
}
