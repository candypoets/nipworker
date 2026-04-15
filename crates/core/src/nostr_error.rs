use thiserror::Error;

#[derive(Debug, Error)]
pub enum NostrError {
    #[error("Network error: {0}")]
    Network(String),
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("Crypto error: {0}")]
    Crypto(String),
    #[error("Relay error: {0}")]
    Relay(String),
    #[error("Types error: {0}")]
    Types(#[from] crate::types::TypesError),
    #[error("Parser error: {0}")]
    Parser(#[from] crate::types::ParserError),
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("Pipeline error: {0}")]
    Pipeline(String),
    #[error("Other error: {0}")]
    Other(String),
}

pub type NostrResult<T> = Result<T, NostrError>;
