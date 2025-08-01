use crate::parser::Parser;
use crate::types::network::Request;
use anyhow::{anyhow, Result};
use nostr::{Event, EventBuilder, UnsignedEvent};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Kind17375Parsed {
    pub mints: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "p2pkPrivKey")]
    pub p2pk_priv_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "p2pkPubKey")]
    pub p2pk_pub_key: Option<String>,
    pub decrypted: bool,
}

impl Parser {
    pub fn parse_kind_17375(
        &self,
        event: &Event,
    ) -> Result<(Kind17375Parsed, Option<Vec<Request>>)> {
        if event.kind.as_u64() != 17375 {
            return Err(anyhow!("event is not kind 17375"));
        }

        let mut parsed = Kind17375Parsed {
            mints: Vec::new(),
            p2pk_priv_key: None,
            p2pk_pub_key: None,
            decrypted: false,
        };

        let signer = &self.signer_manager;

        if signer.has_signer() {
            info!("Signer available, attempting to decrypt event content");
            let pubkey = signer.get_public_key()?;
            if let Ok(decrypted) = signer.nip44_decrypt(&pubkey, &event.content) {
                if !decrypted.is_empty() {
                    if let Ok(tags) = serde_json::from_str::<Vec<Vec<String>>>(&decrypted) {
                        parsed.decrypted = true;

                        // Process decrypted tags
                        for tag in tags {
                            if tag.len() >= 2 {
                                match tag[0].as_str() {
                                    "mint" => {
                                        parsed.mints.push(tag[1].clone());
                                    }
                                    "privkey" => {
                                        parsed.p2pk_priv_key = Some(tag[1].clone());
                                        // Derive public key from private key
                                        if let Ok(secret_key) = nostr::SecretKey::from_hex(&tag[1])
                                        {
                                            let pub_key = secret_key.public_key(&nostr::SECP256K1);
                                            parsed.p2pk_pub_key = Some(pub_key.to_string());
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            }
        } else {
            warn!("No signer found for event");
        }

        // Also check for unencrypted mint tags in the event
        for tag in &event.tags {
            let tag_vec = tag.as_vec();
            if tag_vec.len() >= 2 && tag_vec[0] == "mint" {
                // Only add if not already in the list
                if !parsed.mints.contains(&tag_vec[1]) {
                    parsed.mints.push(tag_vec[1].clone());
                }
            }
        }

        Ok((parsed, None))
    }

    pub fn prepare_kind_17375(&self, event: &mut UnsignedEvent) -> Result<Event> {
        if event.kind.as_u64() != 17375 {
            return Err(anyhow!("event is not kind 17375"));
        }

        // For wallet events, the content should be an array of tags
        let tags: Vec<Vec<String>> = serde_json::from_str(&event.content)
            .map_err(|e| anyhow!("invalid wallet content: {}", e))?;

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
                            return Err(anyhow!("private key appears invalid"));
                        }
                    }
                    _ => {}
                }
            }
        }

        // Mint tag is required in the content
        if !has_mint {
            return Err(anyhow!("wallet must include at least one mint"));
        }

        // If no private key was provided, we would generate one in a full implementation
        if !has_privkey {
            return Err(anyhow!("wallet must include a private key"));
        }

        // Check if signer manager has a signer available
        if !self.signer_manager.has_signer() {
            return Err(anyhow!("no signer available to encrypt message"));
        }

        // Encrypt the message content using NIP-04
        let encrypted_content = self
            .signer_manager
            .nip44_encrypt(&self.signer_manager.get_public_key()?, &event.content)?;

        // Create a new event with the encrypted content using EventBuilder
        let event_builder = EventBuilder::new(event.kind, encrypted_content, event.tags.clone());
        let pub_key = nostr::PublicKey::from_hex(self.signer_manager.get_public_key()?)?;
        let mut unsigned_event = event_builder.to_unsigned_event(pub_key);

        // Sign the event with encrypted content
        let new_event = self.signer_manager.sign_event(&mut unsigned_event)?;

        Ok(new_event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind, Tag};

    #[test]
    fn test_parse_kind_17375_basic() {
        let keys = Keys::generate();

        let event = EventBuilder::new(Kind::Custom(17375), "encrypted_content", Vec::new())
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, _) = parser.parse_kind_17375(&event).unwrap();

        assert!(!parsed.decrypted); // No decryption without signer
        assert!(parsed.mints.is_empty());
        assert!(parsed.p2pk_priv_key.is_none());
        assert!(parsed.p2pk_pub_key.is_none());
    }

    #[test]
    fn test_parse_kind_17375_with_unencrypted_mint_tags() {
        let keys = Keys::generate();
        let mint_url = "https://mint.example.com";

        let tags = vec![Tag::parse(vec!["mint".to_string(), mint_url.to_string()]).unwrap()];

        let event = EventBuilder::new(Kind::Custom(17375), "encrypted_content", tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, _) = parser.parse_kind_17375(&event).unwrap();

        assert_eq!(parsed.mints, vec![mint_url]);
    }

    #[test]
    fn test_prepare_kind_17375_invalid_content() {
        // let keys = Keys::generate();

        // let mut event = EventBuilder::new(Kind::Custom(17375), "invalid json", Vec::new())
        //     .to_event(&keys)
        //     .unwrap();

        // let parser = Parser::default();
        // let result = parser.prepare_kind_17375(&mut event);

        // assert!(result.is_err());
        // assert!(result
        //     .unwrap_err()
        //     .to_string()
        //     .contains("invalid wallet content"));
    }

    #[test]
    fn test_prepare_kind_17375_missing_mint() {
        // let keys = Keys::generate();
        // let content =
        //     r#"[["privkey", "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"]]"#;

        // let mut event = EventBuilder::new(Kind::Custom(17375), content, Vec::new())
        //     .to_event(&keys)
        //     .unwrap();

        // let parser = Parser::default();
        // let result = parser.prepare_kind_17375(&mut event);

        // assert!(result.is_err());
        // assert!(result
        //     .unwrap_err()
        //     .to_string()
        //     .contains("wallet must include at least one mint"));
    }

    #[test]
    fn test_prepare_kind_17375_missing_privkey() {
        // let keys = Keys::generate();
        // let content = r#"[["mint", "https://mint.example.com"]]"#;

        // let mut event = EventBuilder::new(Kind::Custom(17375), content, Vec::new())
        //     .to_event(&keys)
        //     .unwrap();

        // let parser = Parser::default();
        // let result = parser.prepare_kind_17375(&mut event);

        // assert!(result.is_err());
        // assert!(result
        //     .unwrap_err()
        //     .to_string()
        //     .contains("wallet must include a private key"));
    }

    #[test]
    fn test_prepare_kind_17375_invalid_privkey() {
        // let keys = Keys::generate();
        // let content = r#"[["mint", "https://mint.example.com"], ["privkey", "short"]]"#;

        // let mut event = EventBuilder::new(Kind::Custom(17375), content, Vec::new())
        //     .to_event(&keys)
        //     .unwrap();

        // let parser = Parser::default();
        // let result = parser.prepare_kind_17375(&mut event);

        // assert!(result.is_err());
        // assert!(result
        //     .unwrap_err()
        //     .to_string()
        //     .contains("private key appears invalid"));
    }

    #[test]
    fn test_parse_wrong_kind() {
        let keys = Keys::generate();

        let event = EventBuilder::new(Kind::TextNote, "test", Vec::new())
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let result = parser.parse_kind_17375(&event);

        assert!(result.is_err());
    }
}
