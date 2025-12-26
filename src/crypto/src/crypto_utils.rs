//! Cryptographic utilities for the NIPWorker system
//!
//! This module provides common cryptographic functions including
//! hash-to-curve operations used by Cashu proof verification.

use shared::crypto::compute_y_point;

/// Compute Y point from secret using cashu-ts compatible hash_to_curve implementation
///
/// This is a convenience wrapper around the shared crypto module.
/// See `shared::crypto::compute_y_point` for more details.
pub fn hash_to_curve(secret: &str) -> String {
    compute_y_point(secret)
}
