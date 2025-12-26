use crate::parser::{Parser, ParserError, Result};
use tracing::warn;

use shared::{
    generated::nostr::*,
    types::{
        network::Request,
        nostr::{NostrTags, Template},
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

        // Attempt to decrypt using the sender's pubkey
        let sender_pubkey = event.pubkey.to_string();
        match self
            .crypto_client
            .nip44_decrypt(&sender_pubkey, &event.content)
            .await
        {
            Ok(decrypted) => {
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
                                            // Derive public key from private key using k256
                                            if let Ok(secret_key_hex) = hex::decode(&tag[1]) {
                                                if secret_key_hex.len() == 32 {
                                                    if let Ok(signing_key) = k256::schnorr::SigningKey::from_bytes(&secret_key_hex) {
                                                        let pub_key_bytes = signing_key.verifying_key().to_bytes();
                                                        parsed.p2pk_pub_key = Some(hex::encode(&pub_key_bytes));
                                                    }
                                                }
                                            }
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
            }
            Err(e) => {
                warn!("Failed to decrypt event content: {}", e);
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

        // Encrypt the message content using NIP-44; let signer use its own pubkey (empty recipient)
        let encrypted_content = self
            .crypto_client
            .nip44_encrypt("", &template.content)
            .await
            .map_err(|e| ParserError::Crypto(format!("Signer error: {}", e)))?;

        let encrypted_template =
            Template::new(template.kind, encrypted_content, template.tags.clone());

        // Sign the event (SignerClient returns JSON)
        let signed_event_json = self
            .crypto_client
            .sign_event(encrypted_template.to_json())
            .await
            .map_err(|e| ParserError::Crypto(format!("Signer error: {}", e)))?;

        let new_event = Event::from_json(&signed_event_json)?;
        Ok(new_event)
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
