use crate::parser::content::serialize_content_data;
use crate::parser::ContentBlock;
use crate::parser::{content::parse_content, Parser};
use crate::parser::{ParserError, Result};
use crate::signer::interface::SignerManagerInterface;
use crate::types::network::Request;
use crate::types::nostr::{Event, Template};
use crate::utils::request_deduplication::RequestDeduplicator;

use tracing::warn;

// NEW: Imports for FlatBuffers
use shared::generated::nostr::*;

pub struct Kind4Parsed {
    pub parsed_content: Vec<ContentBlock>,
    pub decrypted_content: Option<String>,
    pub chat_id: String,
    pub recipient: String,
}

impl Parser {
    pub fn parse_kind_4(&self, event: &Event) -> Result<(Kind4Parsed, Option<Vec<Request>>)> {
        if event.kind != 4 {
            return Err(ParserError::Other("event is not kind 4".to_string()));
        }

        let mut requests = Vec::new();

        // Get the recipient from the p tag
        let recipient = event
            .tags
            .iter()
            .find_map(|tag| {
                if tag.len() >= 2 && tag[0] == "p" {
                    Some(tag[1].clone())
                } else {
                    None
                }
            })
            .ok_or_else(|| ParserError::Other("no recipient found in DM".to_string()))?;

        let event_pubkey = event.pubkey.to_hex();

        // Request profile information for both sender and recipient
        requests.push(Request {
            authors: vec![event.pubkey.to_hex()],
            kinds: vec![0],
            relays: vec![],
            close_on_eose: true,
            cache_first: true,
            ..Default::default()
        });

        requests.push(Request {
            authors: vec![recipient.clone()],
            kinds: vec![0],
            relays: vec![],
            close_on_eose: true,
            cache_first: true,
            ..Default::default()
        });

        // Create a consistent chat ID by sorting the pubkeys
        let mut chat_participants = vec![event.pubkey.to_hex(), recipient.clone()];
        chat_participants.sort();
        let chat_id = format!("{}_{}", chat_participants[0], chat_participants[1]);

        let mut parsed = Kind4Parsed {
            parsed_content: Vec::new(),
            decrypted_content: None,
            chat_id,
            recipient,
        };

        // Try to decrypt the message using NIP-04
        // The sender is the event author, so we decrypt using their pubkey
        let sender_pubkey = event.pubkey.to_string();

        // Check if we are the sender - if so, use the recipient for decryption
        let decryption_pubkey = if self.signer_manager.has_signer() {
            match self.signer_manager.get_public_key() {
                Ok(our_pubkey) => {
                    if our_pubkey == sender_pubkey {
                        // We are the sender, use recipient for decryption
                        parsed.recipient.clone()
                    } else {
                        // We are not the sender, use sender for decryption
                        sender_pubkey.clone()
                    }
                }
                Err(_) => sender_pubkey.clone(),
            }
        } else {
            sender_pubkey.clone()
        };

        match self
            .signer_manager
            .nip04_decrypt(&decryption_pubkey, &event.content)
        {
            Ok(decrypted) => {
                parsed.decrypted_content = Some(decrypted.clone());

                // Parse the decrypted content into structured blocks
                match parse_content(&decrypted) {
                    Ok(content_blocks) => {
                        parsed.parsed_content = content_blocks
                            .into_iter()
                            .map(|block| ContentBlock {
                                block_type: block.block_type,
                                text: block.text,
                                data: block.data,
                            })
                            .collect();
                    }
                    Err(_) => {
                        // If content parsing fails, create a single text block
                        parsed.parsed_content = vec![ContentBlock {
                            block_type: "text".to_string(),
                            text: decrypted,
                            data: None,
                        }];
                    }
                }
            }
            Err(err) => {
                warn!(
                    "Failed to decrypt kind 4 message from {}: decryption failed: {}",
                    event.pubkey.to_hex(),
                    err
                );
                // If decryption fails, we can't display the content
                // This is normal if we don't have the right keys
            }
        }

        // Deduplicate requests using the utility
        let deduplicated_requests = RequestDeduplicator::deduplicate_requests(&requests);

        Ok((parsed, Some(deduplicated_requests)))
    }

    pub fn prepare_kind_4(&self, event: &Template) -> Result<Event> {
        // Find recipient from p tag
        let recipient = event
            .tags
            .iter()
            .find_map(|tag| {
                if tag.len() >= 2 && tag[0] == "p" {
                    Some(tag[1].clone())
                } else {
                    None
                }
            })
            .ok_or_else(|| ParserError::Other("no recipient found in p tag".to_string()))?;

        // Check if signer manager has a signer available
        if !self.signer_manager.has_signer() {
            return Err(ParserError::Other(
                "no signer available to encrypt message".to_string(),
            ));
        }

        // Encrypt the message content using NIP-04
        let encrypted_content = self
            .signer_manager
            .nip04_encrypt(&recipient, &event.content)?;

        // Create a new event with the encrypted content using EventBuilder
        let new_template = Template::new(event.kind, encrypted_content, event.tags.clone());

        // Sign the event with encrypted content
        let new_event = self.signer_manager.sign_event(&new_template)?;

        Ok(new_event)
    }
}

// NEW: Build the FlatBuffer for Kind4Parsed
pub fn build_flatbuffer<'a, A: flatbuffers::Allocator + 'a>(
    parsed: &Kind4Parsed,
    builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
) -> Result<flatbuffers::WIPOffset<fb::Kind4Parsed<'a>>> {
    // Build parsed_content vector
    let mut parsed_content_offsets = Vec::new();
    for block in &parsed.parsed_content {
        let block_type = builder.create_string(&block.block_type);
        let text = builder.create_string(&block.text);
        let (data_type, data) = match &block.data {
            Some(d) => serialize_content_data(builder, d),
            None => (fb::ContentData::NONE, None),
        };

        let content_block_args = fb::ContentBlockArgs {
            type_: Some(block_type),
            text: Some(text),
            data_type,
            data,
        };
        let content_block_offset = fb::ContentBlock::create(builder, &content_block_args);
        parsed_content_offsets.push(content_block_offset);
    }
    let parsed_content_vector = builder.create_vector(&parsed_content_offsets);

    let decrypted_content = parsed
        .decrypted_content
        .as_ref()
        .map(|s| builder.create_string(s));
    let chat_id = builder.create_string(&parsed.chat_id);
    let recipient = builder.create_string(&parsed.recipient);

    let args = fb::Kind4ParsedArgs {
        parsed_content: Some(parsed_content_vector),
        decrypted_content,
        chat_id: Some(chat_id),
        recipient: Some(recipient),
    };

    let offset = fb::Kind4Parsed::create(builder, &args);

    Ok(offset)
}
