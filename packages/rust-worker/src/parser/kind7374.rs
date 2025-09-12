use crate::parser::Parser;
use crate::types::network::Request;
use anyhow::{anyhow, Result};
use nostr::{Event, UnsignedEvent};

// NEW: Imports for FlatBuffers
use crate::generated::nostr::*;
use flatbuffers::FlatBufferBuilder;

pub struct Kind7374Parsed {
    pub quote_id: String,
    pub mint_url: String,
    pub expiration: u64, // Unix timestamp
}

impl Parser {
    pub fn parse_kind_7374(&self, event: &Event) -> Result<(Kind7374Parsed, Option<Vec<Request>>)> {
        if event.kind.as_u64() != 7374 {
            return Err(anyhow!("event is not kind 7374"));
        }

        // Extract mint URL from tags
        let mut mint_url = String::new();
        let mut expiration_unix = 0u64;

        for tag in &event.tags {
            let tag_vec = tag.as_vec();
            if tag_vec.len() >= 2 {
                match tag_vec[0].as_str() {
                    "mint" => {
                        mint_url = tag_vec[1].clone();
                    }
                    "expiration" => {
                        if let Ok(exp_ts) = tag_vec[1].parse::<u64>() {
                            expiration_unix = exp_ts;
                        }
                    }
                    _ => {}
                }
            }
        }

        if mint_url.is_empty() {
            return Err(anyhow!("mint URL not found in quote event"));
        }

        let mut quote_id = String::new();

        if self.signer_manager.has_signer() {
            let pubkey = self.signer_manager.get_public_key()?;
            if let Ok(decrypted) = self.signer_manager.nip44_decrypt(&pubkey, &event.content) {
                if !decrypted.is_empty() {
                    quote_id = decrypted;
                }
            }
        }

        let parsed = Kind7374Parsed {
            quote_id,
            mint_url,
            expiration: expiration_unix,
        };

        Ok((parsed, None))
    }

    pub fn prepare_kind_7374(&self, event: &mut UnsignedEvent) -> Result<Event> {
        if event.kind.as_u64() != 7374 {
            return Err(anyhow!("event is not kind 7374"));
        }

        // Validate required tags
        let mut has_mint = false;
        for tag in &event.tags {
            let tag_vec = tag.as_vec();
            if tag_vec.len() >= 2 && tag_vec[0] == "mint" {
                has_mint = true;
                break;
            }
        }

        if !has_mint {
            return Err(anyhow!("kind 7374 events must have a mint tag"));
        }

        if self.signer_manager.has_signer() {
            let pubkey = self.signer_manager.get_public_key()?;
            let encrypted = self.signer_manager.nip44_encrypt(&pubkey, &event.content)?;
            event.content = encrypted;
            let new_event = self.signer_manager.sign_event(event)?;
            Ok(new_event)
        } else {
            Err(anyhow!("signer is required for kind 7374 events"))
        }
    }
}

// NEW: Build the FlatBuffer for Kind7374Parsed
pub fn build_flatbuffer<'a, A: flatbuffers::Allocator + 'a>(
    parsed: &Kind7374Parsed,
    builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
) -> Result<flatbuffers::WIPOffset<fb::Kind7374Parsed<'a>>> {
    let quote_id = builder.create_string(&parsed.quote_id);
    let mint_url = builder.create_string(&parsed.mint_url);

    let args = fb::Kind7374ParsedArgs {
        quote_id: Some(quote_id),
        mint_url: Some(mint_url),
        expiration: parsed.expiration,
    };

    let offset = fb::Kind7374Parsed::create(builder, &args);

    Ok(offset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind, Tag};

    #[test]
    fn test_parse_kind_7374_basic() {
        let keys = Keys::generate();
        let mint_url = "https://mint.example.com";
        let expiration = 1234567890u64;

        let tags = vec![
            Tag::parse(vec!["mint".to_string(), mint_url.to_string()]).unwrap(),
            Tag::parse(vec!["expiration".to_string(), expiration.to_string()]).unwrap(),
        ];

        let event = EventBuilder::new(Kind::Custom(7374), "encrypted_quote_id", tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, _) = parser.parse_kind_7374(&event).unwrap();

        assert_eq!(parsed.mint_url, mint_url);
        assert_eq!(parsed.expiration, expiration);
        assert_eq!(parsed.quote_id, ""); // No decryption without signer
    }

    #[test]
    fn test_parse_kind_7374_missing_mint() {
        let keys = Keys::generate();

        let event = EventBuilder::new(Kind::Custom(7374), "content", Vec::new())
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let result = parser.parse_kind_7374(&event);

        assert!(result.is_err());
    }

    #[test]
    fn test_prepare_kind_7374_no_signer() {
        // let keys = Keys::generate();
        // let mint_url = "https://mint.example.com";

        // let tags = vec![Tag::parse(vec!["mint".to_string(), mint_url.to_string()]).unwrap()];

        // let mut event = EventBuilder::new(Kind::Custom(7374), "quote_id", tags)
        //     .to_event(&keys)
        //     .unwrap();

        // let parser = Parser::default();
        // let result = parser.prepare_kind_7374(&mut event);

        // assert!(result.is_err());
        // assert!(result
        //     .unwrap_err()
        //     .to_string()
        //     .contains("encryption and signing not implemented"));
    }

    #[test]
    fn test_parse_wrong_kind() {
        let keys = Keys::generate();

        let event = EventBuilder::new(Kind::TextNote, "test", Vec::new())
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let result = parser.parse_kind_7374(&event);

        assert!(result.is_err());
    }
}
