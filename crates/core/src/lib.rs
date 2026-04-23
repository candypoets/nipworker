pub mod channel;
pub mod generated;
pub mod platform;
pub mod types;
pub mod utils;
pub mod traits;
pub mod service;
pub mod spawn;
pub mod signer_swap;
pub mod nostr_error;
pub mod port;

pub mod crypto_client;
pub mod worker;

#[cfg(feature = "parser")]
pub mod parser;
#[cfg(feature = "parser")]
pub mod pipeline;
#[cfg(feature = "parser")]
pub mod network;
#[cfg(feature = "parser")]
pub mod parser_types;
#[cfg(feature = "parser")]
pub mod parser_utils;

#[cfg(feature = "cache")]
pub mod storage;

#[cfg(feature = "connections")]
pub mod transport;

#[cfg(feature = "crypto")]
pub mod crypto {
    pub mod nostr_crypto;
    pub mod signers;
    pub mod utils;
}

#[cfg(feature = "crypto")]
pub use crypto::utils as crypto_utils;
