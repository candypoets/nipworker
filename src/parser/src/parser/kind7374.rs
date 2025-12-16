use crate::nostr::Template;
use crate::parser::{Parser, ParserError, Result};
use crate::types::network::Request;
use crate::types::nostr::Event;

// NEW: Imports for FlatBuffers
use flatbuffers::FlatBufferBuilder;
use shared::generated::nostr::*;

pub struct Kind7374Parsed {
    pub quote_id: String,
    pub mint_url: String,
    pub expiration: u64, // Unix timestamp
}

impl Parser {
    pub async fn parse_kind_7374(
        &self,
        event: &Event,
    ) -> Result<(Kind7374Parsed, Option<Vec<Request>>)> {
        if event.kind != 7374 {
            return Err(ParserError::Other("event is not kind 7374".to_string()));
        }

        // Extract mint URL and expiration from tags
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

        // Attempt to decrypt the content using our pubkey (if present) via SignerClient (async)
        let mut quote_id = String::new();

        let sender_pubkey = event.pubkey.to_string();
        if let Ok(decrypted) = self
            .signer_client
            .nip44_decrypt(&sender_pubkey, &event.content)
            .await
        {
            if !decrypted.is_empty() {
                quote_id = decrypted;
            }
        }

        let parsed = Kind7374Parsed {
            quote_id,
            mint_url,
            expiration: expiration_unix,
        };

        Ok((parsed, None))
    }

    pub async fn prepare_kind_7374(&self, template: &Template) -> Result<Event> {
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

        let encrypted = self
            .signer_client
            .nip44_encrypt("", &template.content)
            .await
            .map_err(|e| ParserError::Crypto(format!("Signer error: {}", e)))?;

        let encrypted_template = Template::new(template.kind, encrypted, template.tags.clone());

        let signed_event_json = self
            .signer_client
            .sign_event(encrypted_template.to_json())
            .await
            .map_err(|e| ParserError::Crypto(format!("Signer error: {}", e)))?;

        let new_event = Event::from_json(&signed_event_json)?;
        Ok(new_event)
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
