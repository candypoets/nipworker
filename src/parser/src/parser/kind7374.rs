use crate::nostr::Template;
use crate::parser::Parser;
use crate::parser::{ParserError, Result};
use crate::signer::interface::SignerManagerInterface;
use crate::types::network::Request;
use crate::types::nostr::{Event, UnsignedEvent};

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
        if event.kind != 7374 {
            return Err(ParserError::Other("event is not kind 7374".to_string()));
        }

        // Extract mint URL from tags
        let mut mint_url = String::new();
        let mut expiration_unix = 0u64;

        for tag in &event.tags {
            if tag.len() >= 2 {
                match tag[0].as_str() {
                    "mint" => {
                        mint_url = tag[1].clone();
                    }
                    "expiration" => {
                        if let Ok(exp_ts) = tag[1].parse::<u64>() {
                            expiration_unix = exp_ts;
                        }
                    }
                    _ => {}
                }
            }
        }

        if mint_url.is_empty() {
            return Err(ParserError::Other(
                "mint URL not found in quote event".to_string(),
            ));
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

    pub fn prepare_kind_7374(&self, template: &Template) -> Result<Event> {
        if template.kind != 7374 {
            return Err(ParserError::Other("event is not kind 7374".to_string()));
        }

        // Validate required tags
        let mut has_mint = false;
        for tag in &template.tags {
            if tag.len() >= 2 && tag[0] == "mint" {
                has_mint = true;
                break;
            }
        }

        if !has_mint {
            return Err(ParserError::Other(
                "kind 7374 events must have a mint tag".to_string(),
            ));
        }

        if self.signer_manager.has_signer() {
            let pubkey = self.signer_manager.get_public_key()?;
            let encrypted = self
                .signer_manager
                .nip44_encrypt(&pubkey, &template.content)?;
            let encrypted_template = Template::new(template.kind, encrypted, template.tags.clone());
            let new_event = self.signer_manager.sign_event(&encrypted_template)?;
            Ok(new_event)
        } else {
            Err(ParserError::Other(
                "signer is required for kind 7374 events".to_string(),
            ))
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
