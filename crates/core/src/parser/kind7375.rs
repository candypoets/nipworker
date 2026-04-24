use crate::parser::{Parser, ParserError, Result};

use crate::{
    generated::nostr::*,
    types::{network::Request, nostr::{Template, EventId, PublicKey}, Event, Proof, TokenContent},
};
use tracing::warn;

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

        // Attempt to decrypt the content using NIP-44 (self-encrypted wallet event)
        let author = event.pubkey.to_hex();
        let decrypted = if let Some(signer) = &self.signer {
            match signer
                .nip44_decrypt_between(&author, &author, &event.content)
                .await
            {
                Ok(plaintext) => plaintext,
                Err(e) => {
                    warn!("Failed to decrypt kind 7375: {}, treating as plaintext", e);
                    event.content.clone()
                }
            }
        } else {
            event.content.clone()
        };
        if decrypted.is_empty() {
            return Err(ParserError::InvalidContent(
                "Kind 7375 event has empty decrypted content".to_string(),
            ));
        }
        match TokenContent::from_json(&decrypted) {
            Ok(content) => {
                parsed.mint_url = content.mint;
                parsed.proofs = content.proofs;
                parsed.deleted_ids = content.del.unwrap_or_default();
                parsed.decrypted = true;
            }
            Err(e) => {
                return Err(ParserError::InvalidContent(format!(
                    "Failed to parse decrypted kind 7375 content: {}",
                    e
                )));
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
