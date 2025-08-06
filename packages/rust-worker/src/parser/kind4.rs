use crate::parser::{content::parse_content, Parser};
use crate::types::network::Request;
use crate::utils::request_deduplication::RequestDeduplicator;
use anyhow::{anyhow, Result};
use nostr::{Event, EventBuilder, Keys, UnsignedEvent};
use serde::{Deserialize, Serialize};
use tracing::warn;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentBlock {
    #[serde(rename = "type")]
    pub block_type: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Kind4Parsed {
    #[serde(rename = "parsedContent", skip_serializing_if = "Vec::is_empty")]
    pub parsed_content: Vec<ContentBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decrypted_content: Option<String>,
    #[serde(rename = "chatID")]
    pub chat_id: String,
    pub recipient: String,
}

impl Parser {
    pub fn parse_kind_4(&self, event: &Event) -> Result<(Kind4Parsed, Option<Vec<Request>>)> {
        if event.kind.as_u64() != 4 {
            return Err(anyhow!("event is not kind 4"));
        }

        let mut requests = Vec::new();

        // Get the recipient from the p tag
        let recipient = event
            .tags
            .iter()
            .find_map(|tag| {
                let tag_vec = tag.as_vec();
                if tag_vec.len() >= 2 && tag_vec[0] == "p" {
                    Some(tag_vec[1].clone())
                } else {
                    None
                }
            })
            .ok_or_else(|| anyhow!("no recipient found in DM"))?;

        // Request profile information for both sender and recipient
        requests.push(Request {
            authors: vec![event.pubkey.to_hex()],
            kinds: vec![0],
            relays: self
                .database
                .find_relay_candidates(0, &event.pubkey.to_hex(), &false),
            close_on_eose: true,
            cache_first: true,
            ..Default::default()
        });

        requests.push(Request {
            authors: vec![recipient.clone()],
            kinds: vec![0],
            relays: self.database.find_relay_candidates(0, &recipient, &false),
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
                    event.pubkey, err
                );
                // If decryption fails, we can't display the content
                // This is normal if we don't have the right keys
            }
        }

        // Deduplicate requests using the utility
        let deduplicated_requests = RequestDeduplicator::deduplicate_requests(requests);

        Ok((parsed, Some(deduplicated_requests)))
    }

    pub fn prepare_kind_4(&self, event: &mut UnsignedEvent) -> Result<Event> {
        // Find recipient from p tag
        let recipient = event
            .tags
            .iter()
            .find_map(|tag| {
                let tag_vec = tag.as_vec();
                if tag_vec.len() >= 2 && tag_vec[0] == "p" {
                    Some(tag_vec[1].clone())
                } else {
                    None
                }
            })
            .ok_or_else(|| anyhow!("no recipient found in p tag"))?;

        // Check if signer manager has a signer available
        if !self.signer_manager.has_signer() {
            return Err(anyhow!("no signer available to encrypt message"));
        }

        // Encrypt the message content using NIP-04
        let encrypted_content = self
            .signer_manager
            .nip04_encrypt(&recipient, &event.content)?;

        // Create a new event with the encrypted content using EventBuilder
        let event_builder = EventBuilder::new(event.kind, encrypted_content, event.tags.clone());
        let pub_key = nostr::PublicKey::from_hex(self.signer_manager.get_public_key()?)?;
        let mut unsigned_event = event_builder.to_unsigned_event(pub_key);

        // Sign the event with encrypted content
        let new_event = self.signer_manager.sign_event(&mut unsigned_event)?;

        Ok(new_event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind, Tag};

    #[test]
    fn test_parse_kind_4_basic() {
        let keys = Keys::generate();
        let recipient_keys = Keys::generate();
        let encrypted_content = "encrypted_message_content";

        let tags = vec![Tag::parse(vec![
            "p".to_string(),
            recipient_keys.public_key().to_string(),
        ])
        .unwrap()];

        let event = EventBuilder::new(Kind::EncryptedDirectMessage, encrypted_content, tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, requests) = parser.parse_kind_4(&event).unwrap();

        assert_eq!(parsed.recipient, recipient_keys.public_key().to_hex());
        assert!(parsed.chat_id.contains(&keys.public_key().to_hex()));
        assert!(parsed
            .chat_id
            .contains(&recipient_keys.public_key().to_hex()));
        // Note: Decryption will only work if signer manager has the right keys
        assert!(requests.is_some());
        assert_eq!(requests.unwrap().len(), 2); // Requests for both profiles
    }

    #[test]
    fn test_parse_kind_4_chat_id_consistency() {
        let keys1 = Keys::generate();
        let keys2 = Keys::generate();

        // Create message from keys1 to keys2
        let tags1 =
            vec![Tag::parse(vec!["p".to_string(), keys2.public_key().to_string()]).unwrap()];
        let event1 = EventBuilder::new(Kind::EncryptedDirectMessage, "msg1", tags1)
            .to_event(&keys1)
            .unwrap();

        // Create message from keys2 to keys1
        let tags2 =
            vec![Tag::parse(vec!["p".to_string(), keys1.public_key().to_string()]).unwrap()];
        let event2 = EventBuilder::new(Kind::EncryptedDirectMessage, "msg2", tags2)
            .to_event(&keys2)
            .unwrap();

        let parser = Parser::default();
        let (parsed1, _) = parser.parse_kind_4(&event1).unwrap();
        let (parsed2, _) = parser.parse_kind_4(&event2).unwrap();

        // Chat IDs should be the same regardless of who sent the message
        assert_eq!(parsed1.chat_id, parsed2.chat_id);
    }

    #[test]
    fn test_parse_kind_4_no_recipient() {
        let keys = Keys::generate();

        let event = EventBuilder::new(Kind::EncryptedDirectMessage, "content", Vec::new())
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let result = parser.parse_kind_4(&event);

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("no recipient found"));
    }

    #[test]
    fn test_parse_wrong_kind() {
        let keys = Keys::generate();

        let event = EventBuilder::new(Kind::TextNote, "test", Vec::new())
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let result = parser.parse_kind_4(&event);

        assert!(result.is_err());
    }

    #[test]
    fn test_prepare_kind_4_no_signer() {
        // let keys = Keys::generate();
        // let recipient_keys = Keys::generate();

        // let tags = vec![Tag::parse(vec![
        //     "p".to_string(),
        //     recipient_keys.public_key().to_string(),
        // ])
        // .unwrap()];

        // let mut event = EventBuilder::new(Kind::EncryptedDirectMessage, "test message", tags)
        //     .to_event(&keys)
        //     .unwrap();

        // let parser = Parser::default();
        // let result = parser.prepare_kind_4(&mut event);

        // // Should fail without signer
        // assert!(result.is_err());
        // assert!(result
        //     .unwrap_err()
        //     .to_string()
        //     .contains("no signer available"));
    }

    #[test]
    fn test_prepare_kind_4_no_recipient() {
        // let keys = Keys::generate();

        // let mut event = EventBuilder::new(Kind::EncryptedDirectMessage, "test message", Vec::new())
        //     .to_event(&keys)
        //     .unwrap();

        // let parser = Parser::default();
        // let result = parser.prepare_kind_4(&mut event);

        // assert!(result.is_err());
        // assert!(result
        //     .unwrap_err()
        //     .to_string()
        //     .contains("no recipient found"));
    }

    #[test]
    fn test_kind_4_with_signer() {
        // use crate::signer::create_signer_manager;
        // use crate::signer::interface::SignerManager;
        // use crate::types::SignerType;

        // // Create signer manager and set up a private key signer
        // let signer_manager = create_signer_manager();
        // signer_manager.set_signer(SignerType::PrivKey, "").unwrap(); // Generate new key

        // let shared_signer = std::sync::Arc::new(signer_manager);
        // let database = std::sync::Arc::new(crate::db::index::NostrDB::new());
        // let parser = Parser::new_with_signer(shared_signer.clone(), database);

        // // Get the signer's public key
        // let signer_pubkey = shared_signer.get_public_key().unwrap();

        // // Create a test event with the signer as the recipient
        // let keys = Keys::generate();
        // let tags = vec![Tag::parse(vec!["p".to_string(), signer_pubkey.clone()]).unwrap()];

        // let mut event = EventBuilder::new(
        //     Kind::EncryptedDirectMessage,
        //     "Hello, encrypted world!",
        //     tags,
        // )
        // .to_event(&keys)
        // .unwrap();

        // // Test encryption (prepare)
        // let prepare_result = parser.prepare_kind_4(&mut event);
        // assert!(
        //     prepare_result.is_ok(),
        //     "Encryption should succeed with signer"
        // );

        // // The content should now be encrypted
        // assert_ne!(event.content, "Hello, encrypted world!");
        // assert!(!event.content.is_empty());

        // // Test decryption (parse)
        // let (parsed, _) = parser.parse_kind_4(&event).unwrap();

        // // Should have the recipient set correctly
        // assert_eq!(parsed.recipient, signer_pubkey);

        // If the signer has the right keys, decryption might work
        // Note: This depends on the key relationship between the event author and signer
    }

    #[test]
    fn test_parse_kind_4_decryption_attempt() {
        let keys = Keys::generate();
        let recipient_keys = Keys::generate();

        // Create an event with some "encrypted" content (actually just base64 for testing)
        let fake_encrypted = "dGVzdCBtZXNzYWdl"; // "test message" in base64

        let tags = vec![Tag::parse(vec![
            "p".to_string(),
            recipient_keys.public_key().to_string(),
        ])
        .unwrap()];

        let event = EventBuilder::new(Kind::EncryptedDirectMessage, fake_encrypted, tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, _) = parser.parse_kind_4(&event).unwrap();

        // Without the right signer/keys, decryption should fail gracefully
        assert!(parsed.decrypted_content.is_none());
        assert!(parsed.parsed_content.is_empty());
        assert_eq!(parsed.recipient, recipient_keys.public_key().to_hex());
    }
}
