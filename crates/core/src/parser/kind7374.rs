use crate::parser::{Parser, ParserError, Result};

// NEW: Imports for FlatBuffers
use crate::{
    generated::nostr::*,
    types::{network::Request, nostr::{Template, EventId, PublicKey}, Event},
};
use tracing::warn;

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

        // Attempt to decrypt the content using NIP-44 (self-encrypted wallet event)
        let author = event.pubkey.to_hex();
        let quote_id = if event.content.is_empty() {
            return Err(ParserError::InvalidContent(
                "Kind 7374 event has empty decrypted content".to_string(),
            ));
        } else if let Some(signer) = &self.signer {
            match signer
                .nip44_decrypt_between(&author, &author, &event.content)
                .await
            {
                Ok(plaintext) => plaintext,
                Err(e) => {
                    warn!("Failed to decrypt kind 7374: {}, treating as plaintext", e);
                    event.content.clone()
                }
            }
        } else {
            event.content.clone()
        };

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

        let signer = self.signer.as_ref().ok_or_else(|| {
            ParserError::Crypto("encryption not available in parser; signer not configured".into())
        })?;
        let encrypted = signer
            .nip44_encrypt("", &template.content)
            .await
            .map_err(|e| ParserError::Crypto(format!("NIP-44 encrypt error: {}", e)))?;
        let encrypted_template = Template::new(template.kind, encrypted, template.tags.clone());
        self.sign_template(&encrypted_template).await
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
