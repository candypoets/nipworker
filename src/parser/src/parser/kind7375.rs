use crate::parser::{Parser, ParserError, Result};
use tracing::warn;

use shared::{
    generated::nostr::*,
    types::{network::Request, nostr::Template, Event, Proof, TokenContent},
};

pub struct Kind7375Parsed {
    pub mint_url: String,
    pub proofs: Vec<Proof>,
    pub deleted_ids: Vec<String>,
    pub decrypted: bool,
}

impl Parser {
    pub async fn parse_kind_7375(
        &self,
        event: &Event,
    ) -> Result<(Kind7375Parsed, Option<Vec<Request>>)> {
        if event.kind != 7375 {
            return Err(ParserError::Other("event is not kind 7375".to_string()));
        }

        let mut parsed = Kind7375Parsed {
            mint_url: String::new(),
            proofs: Vec::new(),
            deleted_ids: Vec::new(),
            decrypted: false,
        };

        // Attempt to decrypt using the sender's pubkey
        let sender_pubkey = event.pubkey.to_string();
        if let Ok(decrypted) = self
            .signer_client
            .nip44_decrypt(&sender_pubkey, &event.content)
            .await
        {
            if !decrypted.is_empty() {
                match TokenContent::from_json(&decrypted) {
                    Ok(content) => {
                        parsed.mint_url = content.mint;
                        parsed.proofs = content.proofs;
                        parsed.deleted_ids = content.del.unwrap_or_default();
                        parsed.decrypted = true;
                    }
                    Err(e) => {
                        warn!("Failed to parse 7375 token content: {}", e);
                    }
                }
            }
        }

        Ok((parsed, None))
    }

    pub async fn prepare_kind_7375(&self, template: &Template) -> Result<Event> {
        if template.kind != 7375 {
            return Err(ParserError::Other("event is not kind 7375".to_string()));
        }

        // Content must be a valid JSON for TokenContent
        let content: TokenContent = TokenContent::from_json(&template.content)
            .map_err(|e| ParserError::Other(format!("invalid token content: {}", e)))?;

        // Validate content
        if content.mint.is_empty() {
            return Err(ParserError::Other(
                "token content must specify a mint".to_string(),
            ));
        }

        if content.proofs.is_empty() {
            return Err(ParserError::Other(
                "token content must include at least one proof".to_string(),
            ));
        }

        let encrypted_content = self
            .signer_client
            .nip44_encrypt("", &template.content)
            .await
            .map_err(|e| ParserError::Crypto(format!("Signer error: {}", e)))?;

        let encrypted_template =
            Template::new(template.kind, encrypted_content, template.tags.clone());

        let signed_event_json = self
            .signer_client
            .sign_event(encrypted_template.to_json())
            .await
            .map_err(|e| ParserError::Crypto(format!("Signer error: {}", e)))?;

        let event = Event::from_json(&signed_event_json)?;
        Ok(event)
    }
}

// NEW: Build the FlatBuffer for Kind7375Parsed
pub fn build_flatbuffer<'a, A: flatbuffers::Allocator + 'a>(
    parsed: &Kind7375Parsed,
    builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
) -> Result<flatbuffers::WIPOffset<fb::Kind7375Parsed<'a>>> {
    let mint_url = builder.create_string(&parsed.mint_url);

    // Build proofs vector
    let mut proofs_offsets = Vec::new();
    for proof in &parsed.proofs {
        proofs_offsets.push(proof.to_offset(builder));
    }
    let proofs_vector = builder.create_vector(&proofs_offsets);

    // Build deleted_ids vector
    let deleted_ids_offsets: Vec<_> = parsed
        .deleted_ids
        .iter()
        .map(|id| builder.create_string(id))
        .collect();
    let deleted_ids_vector = if deleted_ids_offsets.is_empty() {
        None
    } else {
        Some(builder.create_vector(&deleted_ids_offsets))
    };

    let args = fb::Kind7375ParsedArgs {
        mint_url: Some(mint_url),
        proofs: Some(proofs_vector),
        deleted_ids: deleted_ids_vector,
        decrypted: parsed.decrypted,
    };

    let offset = fb::Kind7375Parsed::create(builder, &args);

    Ok(offset)
}
