use crate::parser::Parser;
use crate::types::network::Request;
use crate::utils::request_deduplication::RequestDeduplicator;
use anyhow::{anyhow, Result};
use nostr::{Event, UnsignedEvent};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofUnion {
    // Placeholder for proof types - would need actual cashu types
    pub data: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Kind9321Parsed {
    pub amount: i32,
    pub recipient: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "eventId")]
    pub event_id: Option<String>,
    #[serde(rename = "mintUrl")]
    pub mint_url: String,
    pub redeemed: bool,
    pub proofs: Vec<ProofUnion>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    #[serde(rename = "isP2PKLocked")]
    pub is_p2pk_locked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "p2pkPubkey")]
    pub p2pk_pubkey: Option<String>,
}

impl Parser {
    pub fn parse_kind_9321(&self, event: &Event) -> Result<(Kind9321Parsed, Option<Vec<Request>>)> {
        if event.kind.as_u64() != 9321 {
            return Err(anyhow!("event is not kind 9321"));
        }

        let mut requests = Vec::new();

        // Get the sender profile for this nutzap
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

        // Extract required tags
        let mut proof_tags = Vec::new();
        let mut mint_tag: Option<Vec<String>> = None;
        let mut recipient_tag: Option<Vec<String>> = None;
        let mut event_tag: Option<Vec<String>> = None;

        for tag in &event.tags {
            let tag_vec = tag.as_vec();
            if tag_vec.len() >= 2 {
                match tag_vec[0].as_str() {
                    "proof" => proof_tags.push(tag_vec.to_vec()),
                    "u" => {
                        if mint_tag.is_none() {
                            mint_tag = Some(tag_vec.to_vec());
                        }
                    }
                    "p" => {
                        if recipient_tag.is_none() {
                            recipient_tag = Some(tag_vec.to_vec());
                            // Get recipient profile
                            requests.push(Request {
                                authors: vec![tag_vec[1].clone()],
                                kinds: vec![0],
                                relays: self.database.find_relay_candidates(0, &tag_vec[1], &false),
                                cache_first: true,
                                ..Default::default()
                            });

                            // Check for spending history events
                            let mut spending_tags = std::collections::HashMap::new();
                            spending_tags.insert("#e".to_string(), vec![event.id.to_hex()]);

                            requests.push(Request {
                                authors: vec![tag_vec[1].clone()],
                                kinds: vec![7376],
                                tags: spending_tags,
                                limit: Some(1),
                                relays: self.database.find_relay_candidates(
                                    7376,
                                    &tag_vec[1],
                                    &false,
                                ),
                                cache_first: true,
                                ..Default::default()
                            });
                        }
                    }
                    "e" => {
                        if event_tag.is_none() {
                            event_tag = Some(tag_vec.to_vec());
                            requests.push(Request {
                                ids: vec![tag_vec[1].clone()],
                                kinds: vec![1],
                                relays: self.database.find_relay_candidates(1, "", &false),
                                cache_first: true,
                                ..Default::default()
                            });
                        }
                    }
                    _ => {}
                }
            }
        }

        // Validate essential tags are present
        if proof_tags.is_empty() || mint_tag.is_none() || recipient_tag.is_none() {
            return Err(anyhow!("missing required tags"));
        }

        let mint_url = mint_tag.as_ref().unwrap()[1].clone();
        let recipient = recipient_tag.as_ref().unwrap()[1].clone();

        // Parse nutzap information
        let mut total = 0i32;
        let mut proofs = Vec::new();
        let mut is_p2pk_locked = false;
        let mut p2pk_pubkey: Option<String> = None;

        for proof_tag in proof_tags {
            // Try to extract proof from the tag
            if let Ok(proof_data) = serde_json::from_str::<Value>(&proof_tag[1]) {
                // Basic amount extraction - would need proper cashu types
                if let Some(amount) = proof_data.get("amount").and_then(|a| a.as_i64()) {
                    total += amount as i32;
                }

                // Check for P2PK locking
                if let Some(secret) = proof_data.get("secret").and_then(|s| s.as_str()) {
                    if secret.contains("P2PK") {
                        is_p2pk_locked = true;
                        // Try to extract pubkey from P2PK secret
                        if let Ok(secret_data) = serde_json::from_str::<Value>(secret) {
                            if let Some(array) = secret_data.as_array() {
                                if array.len() >= 2 && array[0].as_str() == Some("P2PK") {
                                    if let Some(data_obj) = array[1].as_object() {
                                        if let Some(pubkey_str) =
                                            data_obj.get("data").and_then(|d| d.as_str())
                                        {
                                            if p2pk_pubkey.is_none() {
                                                p2pk_pubkey = Some(pubkey_str.to_string());
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                proofs.push(ProofUnion { data: proof_data });
            }
        }

        let result = Kind9321Parsed {
            amount: total,
            recipient,
            mint_url,
            proofs,
            redeemed: false, // Default to not redeemed, will check later
            comment: if event.content.is_empty() {
                None
            } else {
                Some(event.content.clone())
            },
            is_p2pk_locked,
            p2pk_pubkey,
            event_id: event_tag.map(|tag| tag[1].clone()),
        };

        // Deduplicate requests using the utility
        let deduplicated_requests = RequestDeduplicator::deduplicate_requests(requests);

        Ok((result, Some(deduplicated_requests)))
    }

    pub fn prepare_kind_9321(&self, unsigned_event: &mut UnsignedEvent) -> Result<Event> {
        if unsigned_event.kind.as_u64() != 9321 {
            return Err(anyhow!("event is not kind 9321"));
        }

        // Validate required tags
        let mut has_proof = false;
        let mut has_mint = false;
        let mut has_recipient = false;

        for tag in &unsigned_event.tags {
            let tag_vec = tag.as_vec();
            if tag_vec.len() >= 2 {
                match tag_vec[0].as_str() {
                    "proof" => has_proof = true,
                    "u" => has_mint = true,
                    "p" => has_recipient = true,
                    _ => {}
                }
            }
        }

        if !has_proof {
            return Err(anyhow!("kind 9321 must include at least one proof tag"));
        }

        if !has_mint {
            return Err(anyhow!("kind 9321 must include a u tag with mint URL"));
        }

        if !has_recipient {
            return Err(anyhow!("kind 9321 must include a p tag with recipient"));
        }

        self.signer_manager
            .sign_event(unsigned_event)
            .map_err(|e| anyhow!("failed to sign event: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind, Tag};

    #[test]
    fn test_parse_kind_9321_basic() {
        let keys = Keys::generate();
        let recipient_keys = Keys::generate();
        let mint_url = "https://mint.example.com";
        let proof_json = r#"{"amount":100,"secret":"test_secret","C":"test_C","id":"test_id"}"#;

        let tags = vec![
            Tag::parse(vec!["proof".to_string(), proof_json.to_string()]).unwrap(),
            Tag::parse(vec!["u".to_string(), mint_url.to_string()]).unwrap(),
            Tag::parse(vec![
                "p".to_string(),
                recipient_keys.public_key().to_string(),
            ])
            .unwrap(),
        ];

        let event = EventBuilder::new(Kind::Custom(9321), "Test nutzap", tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, requests) = parser.parse_kind_9321(&event).unwrap();

        assert_eq!(parsed.amount, 100);
        assert_eq!(parsed.recipient, recipient_keys.public_key().to_hex());
        assert_eq!(parsed.mint_url, mint_url);
        assert!(!parsed.redeemed);
        assert_eq!(parsed.comment, Some("Test nutzap".to_string()));
        assert!(!parsed.is_p2pk_locked);
        assert!(requests.is_some());
    }

    #[test]
    fn test_parse_kind_9321_missing_required_tags() {
        let keys = Keys::generate();
        let recipient_keys = Keys::generate();

        let tags = vec![
            Tag::parse(vec![
                "p".to_string(),
                recipient_keys.public_key().to_string(),
            ])
            .unwrap(),
            // Missing proof and mint tags
        ];

        let event = EventBuilder::new(Kind::Custom(9321), "Test nutzap", tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let result = parser.parse_kind_9321(&event);

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("missing required tags"));
    }

    #[test]
    fn test_prepare_kind_9321_no_signer() {
        let keys = Keys::generate();
        let recipient_keys = Keys::generate();
        let mint_url = "https://mint.example.com";
        let proof_json = r#"{"amount":100,"secret":"test_secret"}"#;

        let tags = vec![
            Tag::parse(vec!["proof".to_string(), proof_json.to_string()]).unwrap(),
            Tag::parse(vec!["u".to_string(), mint_url.to_string()]).unwrap(),
            Tag::parse(vec![
                "p".to_string(),
                recipient_keys.public_key().to_string(),
            ])
            .unwrap(),
        ];

        let mut event = EventBuilder::new(Kind::Custom(9321), "Test nutzap", tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let result = parser.prepare_kind_9321(&mut event);

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("signing not implemented"));
    }

    #[test]
    fn test_parse_wrong_kind() {
        let keys = Keys::generate();

        let event = EventBuilder::new(Kind::TextNote, "test", Vec::new())
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let result = parser.parse_kind_9321(&event);

        assert!(result.is_err());
    }
}
