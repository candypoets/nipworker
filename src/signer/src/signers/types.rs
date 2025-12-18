type Result<T> = std::result::Result<T, TypesError>;

pub const SECP256K1: k256::Secp256k1 = k256::Secp256k1;

/// Error types for the types module
#[derive(Debug, thiserror::Error)]
pub enum TypesError {
    #[error("Invalid format: {0}")]
    InvalidFormat(String),

    #[error("Missing field: {0}")]
    MissingField(String),

    #[error("Invalid version: {0}")]
    InvalidVersion(i32),

    #[error("Other error: {0}")]
    Other(String),
}
