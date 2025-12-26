//! Nostr cryptographic operations
//!
//! This module provides signing and verification functions for Nostr events.
//! Requires the `crypto` feature to be enabled.

#![cfg(feature = "crypto")]

use crate::types::{Event, EventId, PublicKey, SecretKey, TypesError};
use k256::schnorr::{Signature, SigningKey, VerifyingKey};
use k256::schnorr::signature::{Signer, Verifier};
use sha2::{Digest, Sha256};

type Result<T> = std::result::Result<T, TypesError>;

/// Compute the Nostr event ID (SHA256 hash of serialized event data)
pub fn compute_event_id(
	pubkey: &PublicKey,
	created_at: u64,
	kind: u16,
	tags: &[Vec<String>],
	content: &str,
) -> String {
	let tags_json = format_tags_json(tags);
	let serialized = format!(
		"[0,\"{}\",{},{},{},\"{}\"]",
		pubkey.to_hex(),
		created_at,
		kind,
		tags_json,
		escape_string(content)
	);

	let mut hasher = Sha256::new();
	hasher.update(serialized.as_bytes());
	let result = hasher.finalize();
	hex::encode(result)
}

/// Verify an event's signature
pub fn verify_event_signature(event: &Event) -> Result<()> {
	// Check that all required fields are present
	if event.id.0 == [0u8; 32] {
		return Err(TypesError::InvalidFormat("No ID".to_string()));
	}
	if event.pubkey.0 == [0u8; 32] {
		return Err(TypesError::InvalidFormat("No pubkey".to_string()));
	}
	if event.sig.is_empty() {
		return Err(TypesError::InvalidFormat("No signature".to_string()));
	}

	// Verify signature
	let verifying_key = VerifyingKey::from_bytes(&event.pubkey.0)
		.map_err(|_| TypesError::InvalidFormat("Invalid public key".to_string()))?;
	let signature_bytes = hex::decode(&event.sig)
		.map_err(|_| TypesError::InvalidFormat("Invalid signature hex".to_string()))?;
	let signature = Signature::try_from(signature_bytes.as_slice())
		.map_err(|_| TypesError::InvalidFormat("Invalid signature format".to_string()))?;

	verifying_key
		.verify(&event.id.0, &signature)
		.map_err(|_| TypesError::InvalidFormat("Signature verification failed".to_string()))?;

	Ok(())
}

/// Sign an event with a secret key, returning the signature hex
pub fn sign_event(secret_key: &SecretKey, event_id: &EventId) -> Result<String> {
	let signing_key = SigningKey::from_bytes(&secret_key.0)
		.map_err(|_| TypesError::InvalidFormat("Invalid secret key".to_string()))?;
	let signature = signing_key.sign(&event_id.0);
	Ok(hex::encode(signature.to_bytes()))
}

/// Get the public key from a secret key
pub fn derive_public_key(secret_key: &SecretKey) -> PublicKey {
	let signing_key = SigningKey::from_bytes(&secret_key.0).unwrap();
	let verifying_key = signing_key.verifying_key();
	PublicKey(verifying_key.to_bytes().into())
}

/// Generate a new keypair
pub fn generate_keypair() -> (SecretKey, PublicKey) {
	use k256::elliptic_curve::rand_core::OsRng;

	let signing_key = SigningKey::random(&mut OsRng);
	let secret_bytes: [u8; 32] = signing_key.to_bytes().into();
	let secret = SecretKey(secret_bytes);
	let public = derive_public_key(&secret);
	(secret, public)
}

fn format_tags_json(tags: &[Vec<String>]) -> String {
	let mut result = String::from("[");
	for (i, tag) in tags.iter().enumerate() {
		if i > 0 {
			result.push(',');
		}
		result.push('[');
		for (j, part) in tag.iter().enumerate() {
			if j > 0 {
				result.push(',');
			}
			result.push('"');
			result.push_str(&escape_string(part));
			result.push('"');
		}
		result.push(']');
	}
	result.push(']');
	result
}

fn escape_string(s: &str) -> String {
	let mut result = String::with_capacity(s.len());
	for ch in s.chars() {
		match ch {
			'\\' => result.push_str("\\\\"),
			'"' => result.push_str("\\\""),
			'\n' => result.push_str("\\n"),
			'\r' => result.push_str("\\r"),
			'\t' => result.push_str("\\t"),
			other => result.push(other),
		}
	}
	result
}
