use crate::parser::{Parser, ParserError, Result};
use tracing::warn;

use crate::{
    generated::nostr::*,
    types::{
        network::Request,
        nostr::{NostrTags, Template},
        Event,
    },
};

#[cfg(feature = "crypto")]
fn derive_p2pk_public_key(private_key: &str) -> Result<String> {
    let secret_key = crate::types::SecretKey::from_hex(private_key)?;
    Ok(secret_key.public_key_from_secret().to_hex())
}

#[cfg(not(feature = "crypto"))]
fn derive_p2pk_public_key(_private_key: &str) -> Result<String> {
    Err(ParserError::Crypto(
        "P2PK public-key derivation requires the crypto feature".to_string(),
    ))
}

pub struct Kind17375Parsed {
    pub mints: Vec<String>,
    pub p2pk_priv_key: Option<String>,
    pub p2pk_pub_key: Option<String>,
    pub decrypted: bool,
}

impl Parser {
    pub async fn parse_kind_17375(
        &self,
        event: &Event,
    ) -> Result<(Kind17375Parsed, Option<Vec<Request>>)> {
        if event.kind != 17375 {
            return Err(ParserError::Other("event is not kind 17375".to_string()));
        }

        let mut parsed = Kind17375Parsed {
            mints: Vec::new(),
            p2pk_priv_key: None,
            p2pk_pub_key: None,
            decrypted: false,
        };

        // Plaintext wallet events are accepted for backwards compatibility.
        // Otherwise a configured signer must successfully decrypt the payload:
        // treating a transient remote-signer failure as plaintext emits an empty
        // wallet and makes the pipeline permanently deduplicate the real event.
        let author = event.pubkey.to_hex();
        let decrypted = if NostrTags::from_json(&event.content).is_ok() {
            event.content.clone()
        } else if let Some(signer) = &self.signer {
            signer
                .nip44_decrypt_between(&author, &author, &event.content)
                .await
                .map_err(|e| ParserError::Crypto(format!("Failed to decrypt kind 17375: {}", e)))?
        } else {
            event.content.clone()
        };
        if !decrypted.is_empty() {
            match NostrTags::from_json(&decrypted) {
                Ok(tags) => {
                    parsed.decrypted = true;

                    // Process decrypted tags
                    for tag in tags.0 {
                        if tag.len() >= 2 {
                            match tag[0].as_str() {
                                "mint" => {
                                    parsed.mints.push(tag[1].clone());
                                }
                                "privkey" => {
                                    parsed.p2pk_priv_key = Some(tag[1].clone());
                                    match derive_p2pk_public_key(&tag[1]) {
                                        Ok(public_key) => {
                                            parsed.p2pk_pub_key = Some(public_key);
                                        }
                                        Err(error) => {
                                            warn!(
                                                "Failed to derive kind 17375 P2PK public key: {}",
                                                error
                                            );
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        "Failed to parse decrypted tags for content {}, {}: {}",
                        decrypted, event.content, e
                    );
                }
            }
        }

        // Also check for unencrypted mint tags in the event
        for tag in &event.tags {
            if tag.len() >= 2 && tag[0] == "mint" {
                // Only add if not already in the list
                if !parsed.mints.contains(&tag[1]) {
                    parsed.mints.push(tag[1].clone());
                }
            }
        }

        Ok((parsed, None))
    }

    pub async fn prepare_kind_17375(&self, template: &Template) -> Result<Event> {
        if template.kind != 17375 {
            return Err(ParserError::Other("event is not kind 17375".to_string()));
        }

        // For wallet events, the content should be an array of tags
        let tags: Vec<Vec<String>> = NostrTags::from_json(&template.content)
            .map_err(|e| ParserError::Other(format!("invalid wallet content: {}", e)))?
            .0;

        // Check for required mint tags and validate privkey if present
        let mut has_mint = false;
        let mut has_privkey = false;

        for tag in &tags {
            if tag.len() >= 2 {
                match tag[0].as_str() {
                    "mint" => has_mint = true,
                    "privkey" => {
                        has_privkey = true;
                        // Optionally validate the private key format
                        if tag[1].len() < 32 {
                            return Err(ParserError::Other(
                                "private key appears invalid".to_string(),
                            ));
                        }
                    }
                    _ => {}
                }
            }
        }

        // Mint tag is required in the content
        if !has_mint {
            return Err(ParserError::Other(
                "wallet must include at least one mint".to_string(),
            ));
        }

        // A private key is required in the content
        if !has_privkey {
            return Err(ParserError::Other(
                "wallet must include a private key".to_string(),
            ));
        }

        let signer = self.signer.as_ref().ok_or_else(|| {
            ParserError::Crypto("encryption not available in parser; signer not configured".into())
        })?;
        let encrypted_content = signer
            .nip44_encrypt("", &template.content)
            .await
            .map_err(|e| ParserError::Crypto(format!("NIP-44 encrypt error: {}", e)))?;
        let encrypted_template =
            Template::new(template.kind, encrypted_content, template.tags.clone());
        self.sign_template(&encrypted_template).await
    }
}

// NEW: Build the FlatBuffer for Kind17375Parsed
pub fn build_flatbuffer<'a, A: flatbuffers::Allocator + 'a>(
    parsed: &Kind17375Parsed,
    builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
) -> Result<flatbuffers::WIPOffset<fb::Kind17375Parsed<'a>>> {
    // Build mints vector
    let mints_offsets: Vec<_> = parsed
        .mints
        .iter()
        .map(|mint| builder.create_string(mint))
        .collect();
    let mints_vector = builder.create_vector(&mints_offsets);

    let p2pk_priv_key = parsed
        .p2pk_priv_key
        .as_ref()
        .map(|key| builder.create_string(key));
    let p2pk_pub_key = parsed
        .p2pk_pub_key
        .as_ref()
        .map(|key| builder.create_string(key));

    let args = fb::Kind17375ParsedArgs {
        mints: Some(mints_vector),
        p2pk_priv_key,
        p2pk_pub_key,
        decrypted: parsed.decrypted,
    };

    let offset = fb::Kind17375Parsed::create(builder, &args);

    Ok(offset)
}

#[cfg(all(test, feature = "crypto"))]
mod tests {
    use super::derive_p2pk_public_key;
    use crate::{
        parser::Parser,
        traits::{Signer, SignerError},
        types::{Event, EventId, ParserError, PublicKey},
    };
    use async_trait::async_trait;
    use futures::executor::block_on;
    use std::sync::Arc;

    const EXPECTED_PUBLIC_KEY: &str =
        "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";

    struct UnavailableSigner;

    #[async_trait(?Send)]
    impl Signer for UnavailableSigner {
        async fn get_public_key(&self) -> std::result::Result<String, SignerError> {
            Err(SignerError::Other("unavailable".to_string()))
        }

        async fn sign_event(&self, _event_json: &str) -> std::result::Result<String, SignerError> {
            Err(SignerError::Other("unavailable".to_string()))
        }

        async fn nip04_encrypt(
            &self,
            _peer: &str,
            _plaintext: &str,
        ) -> std::result::Result<String, SignerError> {
            Err(SignerError::Other("unavailable".to_string()))
        }

        async fn nip04_decrypt(
            &self,
            _peer: &str,
            _ciphertext: &str,
        ) -> std::result::Result<String, SignerError> {
            Err(SignerError::Other("unavailable".to_string()))
        }

        async fn nip44_encrypt(
            &self,
            _peer: &str,
            _plaintext: &str,
        ) -> std::result::Result<String, SignerError> {
            Err(SignerError::Other("unavailable".to_string()))
        }

        async fn nip44_decrypt(
            &self,
            _peer: &str,
            _ciphertext: &str,
        ) -> std::result::Result<String, SignerError> {
            Err(SignerError::Other("unavailable".to_string()))
        }

        async fn nip04_decrypt_between(
            &self,
            _sender: &str,
            _recipient: &str,
            _ciphertext: &str,
        ) -> std::result::Result<String, SignerError> {
            Err(SignerError::Other("unavailable".to_string()))
        }

        async fn nip44_decrypt_between(
            &self,
            _sender: &str,
            _recipient: &str,
            _ciphertext: &str,
        ) -> std::result::Result<String, SignerError> {
            Err(SignerError::Other("unavailable".to_string()))
        }
    }

    #[test]
    fn derives_x_only_p2pk_public_key() {
        let private_key = format!("{:064x}", 1);
        let public_key = derive_p2pk_public_key(&private_key).unwrap();

        assert_eq!(public_key, EXPECTED_PUBLIC_KEY);
    }

    #[test]
    fn parsed_wallet_includes_derived_p2pk_public_key() {
        let private_key = format!("{:064x}", 1);
        let event = Event {
            id: EventId([0; 32]),
            pubkey: PublicKey([0; 32]),
            created_at: 1,
            kind: 17375,
            tags: vec![],
            content: format!(
                r#"[["privkey","{}"],["mint","https://mint.example.com"]]"#,
                private_key
            ),
            sig: String::new(),
        };

        let parser = Parser::new(None);
        let (wallet, _) = block_on(parser.parse_kind_17375(&event)).unwrap();

        assert_eq!(wallet.p2pk_priv_key.as_deref(), Some(private_key.as_str()));
        assert_eq!(wallet.p2pk_pub_key.as_deref(), Some(EXPECTED_PUBLIC_KEY));
    }

    #[test]
    fn rejects_invalid_private_key() {
        assert!(derive_p2pk_public_key("not-a-private-key").is_err());
    }

    #[test]
    fn remote_decrypt_failure_is_not_emitted_as_an_empty_wallet() {
        let event = Event {
            id: EventId([0; 32]),
            pubkey: PublicKey([1; 32]),
            created_at: 1,
            kind: 17375,
            tags: vec![],
            content: "encrypted-wallet-payload".to_string(),
            sig: String::new(),
        };
        let parser = Parser::new(Some(Arc::new(UnavailableSigner)));

        let result = block_on(parser.parse_kind_17375(&event));

        assert!(matches!(result, Err(ParserError::Crypto(_))));
    }
}
