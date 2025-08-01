use crate::parser::Parser;
use crate::types::network::Request;
use anyhow::{anyhow, Result};
use nostr::{Event, UnsignedEvent};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MintInfo {
    pub url: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub base_units: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Kind10019Parsed {
    #[serde(skip_serializing_if = "Vec::is_empty", rename = "trustedMints")]
    pub trusted_mints: Vec<MintInfo>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "p2pkPubkey")]
    pub p2pk_pubkey: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", rename = "readRelays")]
    pub read_relays: Vec<String>,
}

impl Parser {
    pub fn parse_kind_10019(
        &self,
        event: &Event,
    ) -> Result<(Kind10019Parsed, Option<Vec<Request>>)> {
        if event.kind.as_u64() != 10019 {
            return Err(anyhow!("event is not kind 10019"));
        }

        let mut parsed = Kind10019Parsed {
            trusted_mints: Vec::new(),
            p2pk_pubkey: None,
            read_relays: Vec::new(),
        };

        // Extract relay, mint, and pubkey tags
        for tag in &event.tags {
            let tag_vec = tag.as_vec();
            if tag_vec.len() >= 2 {
                match tag_vec[0].as_str() {
                    "relay" => {
                        parsed.read_relays.push(tag_vec[1].clone());
                    }
                    "mint" => {
                        let mut mint_info = MintInfo {
                            url: tag_vec[1].clone(),
                            base_units: Vec::new(),
                        };

                        // Extract base units if provided (position 2 and beyond)
                        for i in 2..tag_vec.len() {
                            if !tag_vec[i].is_empty() {
                                mint_info.base_units.push(tag_vec[i].clone());
                            }
                        }

                        parsed.trusted_mints.push(mint_info);
                    }
                    "pubkey" => {
                        parsed.p2pk_pubkey = Some(tag_vec[1].clone());
                    }
                    _ => {}
                }
            }
        }

        // Check if required fields are present
        if parsed.trusted_mints.is_empty() || parsed.p2pk_pubkey.is_none() {
            return Err(anyhow!("missing required mint tags or pubkey tag"));
        }

        Ok((parsed, None))
    }

    pub fn prepare_kind_10019(&self, unsigned_event: &mut UnsignedEvent) -> Result<Event> {
        if unsigned_event.kind.as_u64() != 10019 {
            return Err(anyhow!("event is not kind 10019"));
        }

        // Validate required tags
        let mut has_mint = false;
        let mut has_pubkey = false;

        for tag in &unsigned_event.tags {
            let tag_vec = tag.as_vec();
            if tag_vec.len() >= 2 {
                match tag_vec[0].as_str() {
                    "mint" => has_mint = true,
                    "pubkey" => has_pubkey = true,
                    _ => {}
                }
            }
        }

        if !has_mint {
            return Err(anyhow!("kind 10019 must include at least one mint tag"));
        }

        if !has_pubkey {
            return Err(anyhow!("kind 10019 must include a pubkey tag"));
        }
        self.signer_manager
            .sign_event(unsigned_event)
            .map_err(|e| anyhow!("failed to sign event: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind, Tag};

    #[test]
    fn test_parse_kind_10019_basic() {
        let keys = Keys::generate();
        let mint_url = "https://mint.example.com";
        let pubkey = "npub1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";

        let tags = vec![
            Tag::parse(vec!["mint".to_string(), mint_url.to_string()]).unwrap(),
            Tag::parse(vec!["pubkey".to_string(), pubkey.to_string()]).unwrap(),
        ];

        let event = EventBuilder::new(Kind::Custom(10019), "", tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, _) = parser.parse_kind_10019(&event).unwrap();

        assert_eq!(parsed.trusted_mints.len(), 1);
        assert_eq!(parsed.trusted_mints[0].url, mint_url);
        assert!(parsed.trusted_mints[0].base_units.is_empty());
        assert_eq!(parsed.p2pk_pubkey, Some(pubkey.to_string()));
        assert!(parsed.read_relays.is_empty());
    }

    #[test]
    fn test_parse_kind_10019_with_base_units() {
        let keys = Keys::generate();
        let mint_url = "https://mint.example.com";
        let pubkey = "npub1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";

        let tags = vec![
            Tag::parse(vec![
                "mint".to_string(),
                mint_url.to_string(),
                "sat".to_string(),
                "usd".to_string(),
            ])
            .unwrap(),
            Tag::parse(vec!["pubkey".to_string(), pubkey.to_string()]).unwrap(),
        ];

        let event = EventBuilder::new(Kind::Custom(10019), "", tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, _) = parser.parse_kind_10019(&event).unwrap();

        assert_eq!(parsed.trusted_mints.len(), 1);
        assert_eq!(parsed.trusted_mints[0].base_units, vec!["sat", "usd"]);
    }

    #[test]
    fn test_parse_kind_10019_with_relays() {
        let keys = Keys::generate();
        let mint_url = "https://mint.example.com";
        let pubkey = "npub1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
        let relay_url = "wss://relay.example.com";

        let tags = vec![
            Tag::parse(vec!["mint".to_string(), mint_url.to_string()]).unwrap(),
            Tag::parse(vec!["pubkey".to_string(), pubkey.to_string()]).unwrap(),
            Tag::parse(vec!["relay".to_string(), relay_url.to_string()]).unwrap(),
        ];

        let event = EventBuilder::new(Kind::Custom(10019), "", tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, _) = parser.parse_kind_10019(&event).unwrap();

        assert_eq!(parsed.read_relays, vec![relay_url]);
    }

    #[test]
    fn test_parse_kind_10019_missing_mint() {
        let keys = Keys::generate();
        let pubkey = "npub1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";

        let tags = vec![Tag::parse(vec!["pubkey".to_string(), pubkey.to_string()]).unwrap()];

        let event = EventBuilder::new(Kind::Custom(10019), "", tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let result = parser.parse_kind_10019(&event);

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("missing required mint tags"));
    }

    #[test]
    fn test_parse_kind_10019_missing_pubkey() {
        let keys = Keys::generate();
        let mint_url = "https://mint.example.com";

        let tags = vec![Tag::parse(vec!["mint".to_string(), mint_url.to_string()]).unwrap()];

        let event = EventBuilder::new(Kind::Custom(10019), "", tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let result = parser.parse_kind_10019(&event);

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("missing required"));
    }

    #[test]
    fn test_parse_wrong_kind() {
        let keys = Keys::generate();

        let event = EventBuilder::new(Kind::TextNote, "test", Vec::new())
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let result = parser.parse_kind_10019(&event);

        assert!(result.is_err());
    }
}
