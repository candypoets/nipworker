use crate::nostr::Template;
use crate::types::network::Request;
use crate::types::nostr::{Event, UnsignedEvent};
use crate::{parser::Parser, Proof};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use tracing::warn;

// NEW: Imports for FlatBuffers
use crate::generated::nostr::*;

pub struct Kind7375Parsed {
    pub mint_url: String,
    pub proofs: Vec<Proof>,
    pub deleted_ids: Vec<String>,
    pub decrypted: bool,
}

#[derive(Deserialize)]
struct TokenContent {
    pub mint: String,
    pub proofs: Vec<Proof>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub del: Option<Vec<String>>,
}

impl Parser {
    pub fn parse_kind_7375(&self, event: &Event) -> Result<(Kind7375Parsed, Option<Vec<Request>>)> {
        if event.kind != 7375 {
            return Err(anyhow!("event is not kind 7375"));
        }

        let mut parsed = Kind7375Parsed {
            mint_url: String::new(),
            proofs: Vec::new(),
            deleted_ids: Vec::new(),
            decrypted: false,
        };

        let signer = &self.signer_manager;

        if signer.has_signer() {
            let pubkey = signer.get_public_key()?;
            if let Ok(decrypted) = signer.nip44_decrypt(&pubkey, &event.content) {
                if !decrypted.is_empty() {
                    if let Ok(content) = serde_json::from_str::<TokenContent>(&decrypted) {
                        parsed.mint_url = content.mint;
                        parsed.proofs = content.proofs;
                        parsed.deleted_ids = content.del.unwrap_or_default();
                        parsed.decrypted = true;
                    } else if let Err(e) = serde_json::from_str::<TokenContent>(&decrypted) {
                        warn!("Failed to parse 7375 token content: {}", e);
                    }
                }
            }
        } else {
            warn!("No signer found for event 7375");
        }

        Ok((parsed, None))
    }

    pub fn prepare_kind_7375(&self, template: &Template) -> Result<Event> {
        if template.kind != 7375 {
            return Err(anyhow!("event is not kind 7375"));
        }

        // Content must be a valid JSON for TokenContent
        let _content: TokenContent = serde_json::from_str(&template.content)
            .map_err(|e| anyhow!("invalid token content: {}", e))?;

        // Validate content
        if _content.mint.is_empty() {
            return Err(anyhow!("token content must specify a mint"));
        }

        if _content.proofs.is_empty() {
            return Err(anyhow!("token content must include at least one proof"));
        }

        let signer = &self.signer_manager;

        if !signer.has_signer() {
            return Err(anyhow!("no signer available for encryption"));
        }

        let pubkey = signer.get_public_key()?;
        let encrypted_content = signer.nip44_encrypt(&pubkey, &template.content)?;

        let encrypted_template =
            Template::new(template.kind, encrypted_content, template.tags.clone());

        let event = signer.sign_event(&encrypted_template)?;

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
