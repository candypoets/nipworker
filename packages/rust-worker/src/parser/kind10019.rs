use crate::parser::Parser;
use crate::types::network::Request;
use anyhow::{anyhow, Result};
use nostr::{Event, UnsignedEvent};
use serde::{Deserialize, Serialize};
use tracing::info;

// NEW: Imports for FlatBuffers
use crate::generated::nostr::*;
use flatbuffers::FlatBufferBuilder;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MintInfo {
    pub url: String,
    pub base_units: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Kind10019Parsed {
    #[serde(skip_serializing_if = "Vec::is_empty", rename = "trustedMints")]
    pub trusted_mints: Vec<MintInfo>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "p2pkPubkey")]
    pub p2pk_pubkey: Option<String>,
    #[serde(rename = "readRelays")]
    pub read_relays: Option<Vec<String>>,
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
            read_relays: Some(Vec::new()),
        };

        // Extract relay, mint, and pubkey tags
        for tag in &event.tags {
            let tag_vec = tag.as_vec();
            if tag_vec.len() >= 2 {
                match tag_vec[0].as_str() {
                    "relay" => {
                        if let Some(ref mut relays) = parsed.read_relays {
                            relays.push(tag_vec[1].clone());
                        }
                    }
                    "mint" => {
                        let mut mint_info = MintInfo {
                            url: tag_vec[1].clone(),
                            base_units: Some(Vec::new()),
                        };

                        // Extract base units if provided (position 2 and beyond)
                        for i in 2..tag_vec.len() {
                            if !tag_vec[i].is_empty() {
                                if let Some(ref mut units) = mint_info.base_units {
                                    units.push(tag_vec[i].clone());
                                }
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

// NEW: Build the FlatBuffer for Kind10019Parsed
pub fn build_flatbuffer<'a, A: flatbuffers::Allocator + 'a>(
    parsed: &Kind10019Parsed,
    builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
) -> Result<flatbuffers::WIPOffset<fb::Kind10019Parsed<'a>>> {
    // Build trusted_mints vector
    let mut trusted_mints_offsets = Vec::new();
    for mint in &parsed.trusted_mints {
        let url = builder.create_string(&mint.url);
        let base_units_offsets: Vec<_> = mint
            .base_units
            .iter()
            .flatten()
            .map(|unit| {
                info!("[10019] Adding base_unit: {}", unit);
                builder.create_string(unit)
            })
            .collect();
        let base_units = Some(builder.create_vector(&base_units_offsets));

        let mint_info_args = fb::MintInfoArgs {
            url: Some(url),
            base_units,
        };
        let mint_info_offset = fb::MintInfo::create(builder, &mint_info_args);
        trusted_mints_offsets.push(mint_info_offset);
    }
    let trusted_mints_vector = builder.create_vector(&trusted_mints_offsets);

    let p2pk_pubkey = parsed
        .p2pk_pubkey
        .as_ref()
        .map(|p| builder.create_string(p));

    // Build read_relays vector
    let read_relays_offsets: Vec<_> = parsed
        .read_relays
        .iter()
        .flatten()
        .map(|relay| builder.create_string(relay))
        .collect();
    let read_relays_vector = if read_relays_offsets.is_empty() {
        None
    } else {
        Some(builder.create_vector(&read_relays_offsets))
    };

    let args = fb::Kind10019ParsedArgs {
        trusted_mints: Some(trusted_mints_vector),
        p2pk_pubkey,
        read_relays: read_relays_vector,
    };

    let offset = fb::Kind10019Parsed::create(builder, &args);

    Ok(offset)
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
        assert_eq!(parsed.trusted_mints[0].base_units, Some(Vec::new()));
        assert_eq!(parsed.p2pk_pubkey, Some(pubkey.to_string()));
        assert_eq!(parsed.read_relays, Some(Vec::new()));
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
        assert_eq!(
            parsed.trusted_mints[0].base_units,
            Some(vec!["sat".to_string(), "usd".to_string()])
        );
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

        assert_eq!(parsed.read_relays, Some(vec![relay_url.to_string()]));
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
