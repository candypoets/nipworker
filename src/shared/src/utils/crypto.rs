//! Shared cryptographic utilities
//!
//! This module provides common cryptographic functions used across
//! different components of the NIPWorker system.

use hex;
use k256::{
    elliptic_curve::{sec1::{FromEncodedPoint, ToEncodedPoint}, PrimeField},
    AffinePoint, EncodedPoint, ProjectivePoint, PublicKey, Scalar,
};
use rustc_hash::FxHashMap;
use sha2::{Digest, Sha256};
use crate::types::proof::{DleqProof, Proof};
use crate::types::ParserError;

type Result<T> = std::result::Result<T, ParserError>;

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

/// Verify a Cashu proof with DLEQ signature against mint keys
///
/// This function implements NUT-12 proof verification using Chaum-Pedersen DLEQ
pub fn verify_proof_dleq_with_keys(proof: &Proof, keys_map: &FxHashMap<u64, String>) -> bool {
    let dleq: &DleqProof = match &proof.dleq {
        Some(d) => d,
        None => return false,
    };

    let key_hex = match keys_map.get(&proof.amount) {
        Some(k) => k,
        None => return false,
    };

    let a = match point_from_hex_unchecked(key_hex) {
        Some(p) => p,
        None => return false,
    };

    let c = match point_from_hex_unchecked(&proof.c) {
        Some(p) => p,
        None => return false,
    };

    let r = match parse_scalar_hex(&dleq.r.as_deref().unwrap_or("00")) {
        Some(s) => s,
        None => return false,
    };
    let e = match parse_scalar_hex(&dleq.e) {
        Some(s) => s,
        None => return false,
    };
    let s = match parse_scalar_hex(&dleq.s) {
        Some(sv) => sv,
        None => return false,
    };

    // Y = hash_to_curve(secret) (NUT-00)
    let y = match hash_to_curve_point(proof.secret.as_bytes()) {
        Some(p) => p,
        None => return false,
    };

    // Reblind to reconstruct Carol-side B' and C' (NUT-12):
    // B' = Y + r*G
    // C' = C + r*A
    let r_g = ProjectivePoint::from(AffinePoint::GENERATOR) * r;

    let bp = y + r_g;

    let cp = c + (a * r);

    // Verify Chaum-Pedersen DLEQ (NUT-12):
    // R1 = s*G - e*A
    // R2 = s*B' - e*C'
    // e' = H(R1, R2, A, C')  (uncompressed hex concatenation)
    let s_g = ProjectivePoint::from(AffinePoint::GENERATOR) * s;

    let e_a = a * e;

    let r1 = s_g + (-e_a);

    let s_b = bp * s;

    let e_c = cp * e;

    let r2 = s_b + (-e_c);

    let e_prime = compute_challenge_e_nut12(&r1, &r2, &a, &cp);

    let valid = e == e_prime;
    valid
}

/// Verify proof with JSON payload containing proof and mint keys
///
/// Payload format: {"proof": {...}, "mint_keys": {"1": "02...", "2": "03...", ...}}
pub fn verify_proof_dleq(payload: &str) -> Result<bool> {
    // Parse JSON payload to extract proof and mint_keys
    let (proof, keys_map) = parse_verification_payload(payload)?;
    Ok(verify_proof_dleq_with_keys(&proof, &keys_map))
}

pub fn verify_proof_dleq_string(payload: &str) -> std::result::Result<bool, String> {
    verify_proof_dleq(payload).map_err(|e| e.to_string())
}

pub fn parse_verification_payload(payload: &str) -> Result<(Proof, FxHashMap<u64, String>)> {
    // Split payload by delimiter
    let parts: Vec<&str> = payload.split("|||").collect();
    
    if parts.len() != 2 {
        return Err(ParserError::Other("Invalid payload format: expected proof|||mint_keys".into()));
    }
    
    let proof_json = parts[0];
    let mint_keys_json = parts[1];
    
    // Use Proof::from_json directly
    let proof = Proof::from_json(proof_json)
        .map_err(|e| ParserError::Parse(e.to_string()))?;
    
    // Parse mint keys
    let keys_map = parse_mint_keys_json(mint_keys_json)?;
    
    Ok((proof, keys_map))
}

fn parse_mint_keys_json(content: &str) -> Result<FxHashMap<u64, String>> {
    let mut map = FxHashMap::default();

    if content.trim().is_empty() {
        return Ok(map);
    }

    for pair in content.split(',') {
        let parts: Vec<&str> = pair.splitn(2, ':').collect();
        if parts.len() != 2 {
            continue;
        }

        let amount_str = parts[0].trim().trim_matches('"');
        let key_hex = parts[1].trim().trim_matches('"');

        if let Ok(amount) = amount_str.parse::<u64>() {
            map.insert(amount, key_hex.to_string());
        }
    }

    Ok(map)
}

// Helper functions for proof verification

fn parse_scalar_hex(s: &str) -> Option<Scalar> {
    let s = s.strip_prefix("0x").unwrap_or(s).trim();
    let bytes = hex::decode(s).ok()?;
    let mut be = [0u8; 32];
    if bytes.len() > 32 {
        be.copy_from_slice(&bytes[bytes.len() - 32..]);
    } else {
        be[32 - bytes.len()..].copy_from_slice(&bytes);
    }
    // Use from_repr (no reduction); practically always succeeds for 32-byte values from SHA-256 or valid scalars
    Option::<Scalar>::from(Scalar::from_repr(be.into()))
}

fn point_from_hex_unchecked(hex_str: &str) -> Option<ProjectivePoint> {
    let s = hex_str.strip_prefix("0x").unwrap_or(hex_str).trim();
    let bytes = hex::decode(s).ok()?;
    let ep = EncodedPoint::from_bytes(&bytes).ok()?;
    let aff_opt: Option<AffinePoint> = AffinePoint::from_encoded_point(&ep).into();
    aff_opt.map(ProjectivePoint::from)
}

fn point_to_uncompressed_hex(p: &ProjectivePoint) -> String {
    let aff = AffinePoint::from(*p);
    let ep = aff.to_encoded_point(false); // uncompressed 65 bytes
    hex::encode(ep.as_bytes())
}

// NUT-12: e = SHA256(hex(uncompressed(R1)) || hex(uncompressed(R2)) || hex(uncompressed(A)) || hex(uncompressed(C')))
fn compute_challenge_e_nut12(
    R1: &ProjectivePoint,
    R2: &ProjectivePoint,
    A: &ProjectivePoint,
    Cp: &ProjectivePoint,
) -> Scalar {
    let r1_hex = point_to_uncompressed_hex(R1);
    let r2_hex = point_to_uncompressed_hex(R2);
    let a_hex = point_to_uncompressed_hex(A);
    let c_hex = point_to_uncompressed_hex(Cp);

    let mut hasher = Sha256::new();
    hasher.update(r1_hex.as_bytes());
    hasher.update(r2_hex.as_bytes());
    hasher.update(a_hex.as_bytes());
    hasher.update(c_hex.as_bytes());
    let digest = hasher.finalize();

    let mut be = [0u8; 32];
    be.copy_from_slice(&digest);
    // Use from_repr instead of reduced
    Option::<Scalar>::from(Scalar::from_repr(be.into()))
        .expect("digest should almost always be < n for secp256k1")
}

// NUT-00 hash_to_curve (Cashu)
// Y = PublicKey('02' || SHA256(SHA256(DOMAIN_SEPARATOR||x) || counter_le))
// Find first valid point
fn hash_to_curve_point(secret: &[u8]) -> Option<ProjectivePoint> {
    const DOMAIN_SEPARATOR: &[u8] = b"Secp256k1_HashToCurve_Cashu_";

    let mut h = Sha256::new();
    h.update(DOMAIN_SEPARATOR);
    h.update(secret);
    let msg_hash = h.finalize();

    for counter in 0u32..=u32::MAX {
        let mut hh = Sha256::new();
        hh.update(&msg_hash);
        hh.update(&counter.to_le_bytes());
        let hash = hh.finalize();

        let mut bytes = [0u8; 33];
        bytes[0] = 0x02;
        bytes[1..].copy_from_slice(&hash);

        if let Ok(ep) = EncodedPoint::from_bytes(&bytes[..]) {
            let aff_opt: Option<AffinePoint> = AffinePoint::from_encoded_point(&ep).into();
            if let Some(aff) = aff_opt {
                return Some(ProjectivePoint::from(aff));
            }
        }
        if counter == 65535 {
            // Also try with 0x03 prefix
            bytes[0] = 0x03;
            if let Ok(ep) = EncodedPoint::from_bytes(&bytes[..]) {
                let aff_opt: Option<AffinePoint> = AffinePoint::from_encoded_point(&ep).into();
                if let Some(aff) = aff_opt {
                    return Some(ProjectivePoint::from(aff));
                }
            }
        }
    }
    None
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