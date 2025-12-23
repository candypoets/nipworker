//! Shared cryptographic utilities
//!
//! This module provides common cryptographic functions used across
//! different components of the NIPWorker system.

use hex;
use k256::{elliptic_curve::sec1::ToEncodedPoint, PublicKey};
use sha2::{Digest, Sha256};

/// Compute Y point from secret using cashu-ts compatible hash_to_curve implementation
///
/// This function implements the hash_to_curve algorithm used by Cashu
/// to derive a secp256k1 point from a secret string.
///
/// # Arguments
/// * `secret` - The secret string to hash to a curve point
///
/// # Returns
/// Hex-encoded compressed public key (33 bytes with 0x02 or 0x03 prefix)
///
/// # Panics
/// If no valid point is found after 65536 iterations (should never happen in practice)
pub fn compute_y_point(secret: &str) -> String {
    const DOMAIN_SEPARATOR: &[u8] = b"Secp256k1_HashToCurve_Cashu_";

    // First hash: DOMAIN_SEPARATOR || secret
    let mut hasher = Sha256::new();
    hasher.update(DOMAIN_SEPARATOR);
    hasher.update(secret.as_bytes());
    let msg_hash = hasher.finalize();

    // Counter loop to find valid point
    const MAX_ITERATIONS: u32 = 65536; // 2^16
    for counter in 0..MAX_ITERATIONS {
        let mut hasher = Sha256::new();
        hasher.update(&msg_hash);
        hasher.update(&counter.to_le_bytes()); // little endian as per spec
        let hash = hasher.finalize();

        // Try to create point with 0x02 prefix (compressed)
        let mut point_bytes = Vec::with_capacity(33);
        point_bytes.push(0x02);
        point_bytes.extend_from_slice(&hash);

        // Try to parse as a valid secp256k1 point
        if let Ok(public_key) = PublicKey::from_sec1_bytes(&point_bytes) {
            let encoded_point = public_key.to_encoded_point(true);
            return hex::encode(encoded_point.as_bytes());
        }
    }

    panic!("No valid point found after 65536 iterations");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_y_point() {
        let secret = "test_secret";
        let y_point = compute_y_point(secret);
        
        // Y point should be 66 hex chars (33 bytes * 2)
        assert_eq!(y_point.len(), 66);
        
        // Should start with 02 or 03 (compressed point prefix)
        assert!(y_point.starts_with("02") || y_point.starts_with("03"));
        
        // Should be valid hex
        hex::decode(&y_point).expect("Y point should be valid hex");
    }

    #[test]
    fn test_compute_y_point_deterministic() {
        let secret = "deterministic_test";
        let y_point1 = compute_y_point(secret);
        let y_point2 = compute_y_point(secret);
        
        // Should be deterministic
        assert_eq!(y_point1, y_point2);
    }

    #[test]
    fn test_compute_y_point_different_secrets() {
        let y_point1 = compute_y_point("secret1");
        let y_point2 = compute_y_point("secret2");
        
        // Different secrets should produce different points
        assert_ne!(y_point1, y_point2);
    }
}