use crate::parser::{Parser, ParserError, Result};
use tracing::warn;

use crate::{
    generated::nostr::*,
    types::{
        network::Request,
        nostr::{NostrTags, Template, EventId, PublicKey},
        Event,
    },
};

pub struct Kind17375Parsed {
    pub mints: Vec<String>,
    pub p2pk_priv_key: Option<String>,
    pub p2pk_pub_key: Option<String>,
    pub decrypted: bool,
}

impl Parser {
    pub async fn parse_kind_17375(
        &self,
        event: &Event,
    ) -> Result<(Kind17375Parsed, Option<Vec<Request>>)> {
        if event.kind != 17375 {
            return Err(ParserError::Other("event is not kind 17375".to_string()));
        }

        let mut parsed = Kind17375Parsed {
            mints: Vec::new(),
            p2pk_priv_key: None,
            p2pk_pub_key: None,
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
                    warn!("Failed to decrypt kind 17375: {}, treating as plaintext", e);
                    event.content.clone()
                }
            }
        } else {
            event.content.clone()
        };
        if !decrypted.is_empty() {
            match NostrTags::from_json(&decrypted) {
                Ok(tags) => {
                    parsed.decrypted = true;

                    // Process decrypted tags
                    for tag in tags.0 {
                        if tag.len() >= 2 {
                            match tag[0].as_str() {
                                "mint" => {
                                    parsed.mints.push(tag[1].clone());
                                }
                                "privkey" => {
                                    parsed.p2pk_priv_key = Some(tag[1].clone());
                                    // TODO: Derive public key from private key via crypto worker if needed
                                    parsed.p2pk_pub_key = Some(String::new());
                                }
                                _ => {}
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        "Failed to parse decrypted tags for content {}, {}: {}",
                        decrypted, event.content, e
                    );
                }
            }
        }

        // Also check for unencrypted mint tags in the event
        for tag in &event.tags {
            if tag.len() >= 2 && tag[0] == "mint" {
                // Only add if not already in the list
                if !parsed.mints.contains(&tag[1]) {
                    parsed.mints.push(tag[1].clone());
                }
            }
        }

        Ok((parsed, None))
    }

    pub async fn prepare_kind_17375(&self, template: &Template) -> Result<Event> {
        if template.kind != 17375 {
            return Err(ParserError::Other("event is not kind 17375".to_string()));
        }

        // For wallet events, the content should be an array of tags
        let tags: Vec<Vec<String>> = NostrTags::from_json(&template.content)
            .map_err(|e| ParserError::Other(format!("invalid wallet content: {}", e)))?
            .0;

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
                            return Err(ParserError::Other(
                                "private key appears invalid".to_string(),
                            ));
                        }
                    }
                    _ => {}
                }
            }
        }

        // Mint tag is required in the content
        if !has_mint {
            return Err(ParserError::Other(
                "wallet must include at least one mint".to_string(),
            ));
        }

        // A private key is required in the content
        if !has_privkey {
            return Err(ParserError::Other(
                "wallet must include a private key".to_string(),
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

// NEW: Build the FlatBuffer for Kind17375Parsed
pub fn build_flatbuffer<'a, A: flatbuffers::Allocator + 'a>(
    parsed: &Kind17375Parsed,
    builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
) -> Result<flatbuffers::WIPOffset<fb::Kind17375Parsed<'a>>> {
    // Build mints vector
    let mints_offsets: Vec<_> = parsed
        .mints
        .iter()
        .map(|mint| builder.create_string(mint))
        .collect();
    let mints_vector = builder.create_vector(&mints_offsets);

    let p2pk_priv_key = parsed
        .p2pk_priv_key
        .as_ref()
        .map(|key| builder.create_string(key));
    let p2pk_pub_key = parsed
        .p2pk_pub_key
        .as_ref()
        .map(|key| builder.create_string(key));

    let args = fb::Kind17375ParsedArgs {
        mints: Some(mints_vector),
        p2pk_priv_key,
        p2pk_pub_key,
        decrypted: parsed.decrypted,
    };

    let offset = fb::Kind17375Parsed::create(builder, &args);

    Ok(offset)
}
