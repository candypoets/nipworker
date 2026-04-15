/// Configuration for a NIP-46 remote signer session (Nostr Connect).
#[derive(Clone, Debug)]
pub struct Nip46Config {
    /// Remote signer public key (hex, x-only)
    pub remote_signer_pubkey: String,
    /// Relays to use for the NIP-46 RPC traffic
    pub relays: Vec<String>,
    /// Prefer NIP-44 (v2) encryption. If false, attempt NIP-04 as fallback.
    pub use_nip44: bool,
    /// Optional app name or label to include as a tag in requests
    pub app_name: Option<String>,
    /// Expected secret for QR code validation (optional)
    pub expected_secret: Option<String>,
}
