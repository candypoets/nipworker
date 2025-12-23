//! Cryptographic utilities for the NIPWorker system
//!
//! This module provides common cryptographic functions including
//! hash-to-curve operations used by Cashu proof verification.

use shared::crypto::compute_y_point;
use wasm_bindgen::prelude::*;

/// Compute Y point from secret using cashu-ts compatible hash_to_curve implementation
///
/// This is a convenience wrapper around the shared crypto module.
/// See `shared::crypto::compute_y_point` for more details.
#[wasm_bindgen]
pub fn hash_to_curve(secret: &str) -> String {
    compute_y_point(secret)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_to_curve() {
        let secret = "test_secret";
        let y_point = hash_to_curve(secret);

        // Y point should be 66 hex chars (33 bytes * 2)
        assert_eq!(y_point.len(), 66);

        // Should start with 02 or 03 (compressed point prefix)
        assert!(y_point.starts_with("02") || y_point.starts_with("03"));
    }

    #[test]
    fn test_hash_to_curve_deterministic() {
        let secret = "deterministic_test";
        let y_point1 = hash_to_curve(secret);
        let y_point2 = hash_to_curve(secret);

        // Should be deterministic
        assert_eq!(y_point1, y_point2);
    }
}
