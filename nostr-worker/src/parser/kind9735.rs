use crate::parser::Parser;
use crate::types::network::Request;
use crate::utils::request_deduplication::RequestDeduplicator;
use anyhow::{anyhow, Result};
use nostr::Event;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZapRequest {
    pub kind: u64,
    pub pubkey: String,
    pub content: String,
    pub tags: Vec<Vec<String>>,
    #[serde(alias = "sig")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Kind9735Parsed {
    pub id: String,
    pub amount: i32,
    pub content: String,
    pub bolt11: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preimage: Option<String>,
    pub sender: String,
    pub recipient: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "eventCoordinate")]
    pub event_coordinate: Option<String>,
    pub timestamp: u64,
    pub valid: bool,
    pub description: ZapRequest,
}

impl Parser {
    pub fn parse_kind_9735(&self, event: &Event) -> Result<(Kind9735Parsed, Option<Vec<Request>>)> {
        if event.kind.as_u64() != 9735 {
            return Err(anyhow!("event is not kind 9735"));
        }

        let mut requests = Vec::new();

        // Get the sender profile for this zap
        requests.push(Request {
            authors: vec![event.pubkey.to_hex()],
            kinds: vec![0],
            relays: self
                .database
                .find_relay_candidates(0, &event.pubkey.to_hex(), &false),
            cache_first: true,
            ..Default::default()
        });

        // Extract tags
        let mut p_tag: Option<Vec<String>> = None;
        let mut e_tag: Option<Vec<String>> = None;
        let mut a_tag: Option<Vec<String>> = None;
        let mut bolt11_tag: Option<Vec<String>> = None;
        let mut description_tag: Option<Vec<String>> = None;
        let mut preimage_tag: Option<Vec<String>> = None;
        let mut sender_tag: Option<Vec<String>> = None;

        for tag in &event.tags {
            let tag_vec = tag.as_vec();
            if tag_vec.len() >= 2 {
                match tag_vec[0].as_str() {
                    "p" => p_tag = Some(tag_vec.to_vec()),
                    "e" => e_tag = Some(tag_vec.to_vec()),
                    "a" => a_tag = Some(tag_vec.to_vec()),
                    "bolt11" => bolt11_tag = Some(tag_vec.to_vec()),
                    "description" => description_tag = Some(tag_vec.to_vec()),
                    "preimage" => preimage_tag = Some(tag_vec.to_vec()),
                    "P" => sender_tag = Some(tag_vec.to_vec()), // Capital P for sender
                    _ => {}
                }
            }
        }

        // Require mandatory tags
        if p_tag.is_none() || bolt11_tag.is_none() || description_tag.is_none() {
            return Err(anyhow!("missing required tags"));
        }

        let recipient = p_tag.as_ref().unwrap()[1].clone();
        let bolt11 = bolt11_tag.as_ref().unwrap()[1].clone();
        let description_str = description_tag.as_ref().unwrap()[1].clone();

        // Parse the zap request from the description tag
        let zap_request: ZapRequest = serde_json::from_str(&description_str)
            .map_err(|e| anyhow!("failed to parse zap request description: {}", e))?;

        // Validate that the zap request is properly formed
        if zap_request.kind != 9734 || zap_request.tags.is_empty() {
            return Err(anyhow!("invalid zap request"));
        }

        // Extract amount from bolt11 invoice or zap request
        let mut amount = 0i32;

        // First check if there's an amount tag in the zap request
        if let Some(amount_tag) = find_tag_in_vec(&zap_request.tags, "amount") {
            if amount_tag.len() >= 2 {
                if let Ok(amt_int) = amount_tag[1].parse::<i64>() {
                    amount = (amt_int / 1000) as i32; // Convert from millisats to sats
                }
            }
        }
        // TODO: If no amount tag, try to decode bolt11 invoice

        // Determine sender
        let sender = if let Some(sender_tag) = sender_tag {
            sender_tag[1].clone()
        } else {
            zap_request.pubkey.clone()
        };

        // Extract relay hints from the zap request
        let mut zapper_relay_hints = Vec::new();
        if let Some(relays_tag) = find_tag_in_vec(&zap_request.tags, "relays") {
            if relays_tag.len() > 1 {
                zapper_relay_hints = relays_tag[1..].to_vec();
            }
        }

        // Try to find the zapper profile
        requests.push(Request {
            authors: vec![event.pubkey.to_hex()],
            kinds: vec![0],
            limit: Some(1),
            relays: {
                let mut relays =
                    self.database
                        .find_relay_candidates(0, &event.pubkey.to_hex(), &false);
                relays.extend(zapper_relay_hints);
                relays
            },
            cache_first: true,
            ..Default::default()
        });

        // Create the parsed zap receipt
        let mut receipt = Kind9735Parsed {
            id: event.id.to_hex(),
            amount,
            content: zap_request.content.clone(),
            bolt11,
            sender,
            recipient: recipient.clone(),
            timestamp: event.created_at.as_u64(),
            valid: true, // We'll validate below
            description: zap_request,
            preimage: preimage_tag.map(|tag| tag[1].clone()),
            event: e_tag.map(|tag| tag[1].clone()),
            event_coordinate: a_tag.map(|tag| tag[1].clone()),
        };

        // Perform basic validation
        // 1. The zap request should have the same recipient as the receipt
        if let Some(request_p_tag) = find_tag_in_vec(&receipt.description.tags, "p") {
            if request_p_tag.len() >= 2 && request_p_tag[1] != receipt.recipient {
                receipt.valid = false;
            }
        } else {
            receipt.valid = false;
        }

        // 2. If the receipt has an event ID, the request should also have it
        if let Some(ref event_id) = receipt.event {
            if let Some(request_e_tag) = find_tag_in_vec(&receipt.description.tags, "e") {
                if request_e_tag.len() < 2 || request_e_tag[1] != *event_id {
                    receipt.valid = false;
                }
            } else {
                receipt.valid = false;
            }
        }

        // 3. If the receipt has an event coordinate, the request should also have it
        if let Some(ref event_coordinate) = receipt.event_coordinate {
            if let Some(request_a_tag) = find_tag_in_vec(&receipt.description.tags, "a") {
                if request_a_tag.len() < 2 || request_a_tag[1] != *event_coordinate {
                    receipt.valid = false;
                }
            } else {
                receipt.valid = false;
            }
        }

        // Deduplicate requests using the utility
        let deduplicated_requests = RequestDeduplicator::deduplicate_requests(requests);

        Ok((receipt, Some(deduplicated_requests)))
    }
}

// Helper function to find a tag by name in a vec of vec of strings
fn find_tag_in_vec<'a>(tags: &'a [Vec<String>], name: &str) -> Option<&'a Vec<String>> {
    tags.iter().find(|tag| !tag.is_empty() && tag[0] == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind, Tag};

    #[test]
    fn test_parse_kind_9735_basic() {
        let keys = Keys::generate();
        let recipient_keys = Keys::generate();
        let bolt11 = "lnbc1000n1..."; // Mock bolt11

        let zap_request = ZapRequest {
            kind: 9734,
            pubkey: keys.public_key().to_hex(),
            content: "Great post!".to_string(),
            tags: vec![
                vec!["p".to_string(), recipient_keys.public_key().to_hex()],
                vec!["amount".to_string(), "1000000".to_string()], // 1000 sats in millisats
            ],
            signature: Some("mock_signature".to_string()),
        };

        let description = serde_json::to_string(&zap_request).unwrap();

        let tags = vec![
            Tag::parse(vec![
                "p".to_string(),
                recipient_keys.public_key().to_string(),
            ])
            .unwrap(),
            Tag::parse(vec!["bolt11".to_string(), bolt11.to_string()]).unwrap(),
            Tag::parse(vec!["description".to_string(), description]).unwrap(),
        ];

        let event = EventBuilder::new(Kind::ZapReceipt, "", tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, requests) = parser.parse_kind_9735(&event).unwrap();

        assert_eq!(parsed.amount, 1000); // 1000000 millisats = 1000 sats
        assert_eq!(parsed.recipient, recipient_keys.public_key().to_hex());
        assert_eq!(parsed.bolt11, bolt11);
        assert_eq!(parsed.content, "Great post!");
        assert!(parsed.valid);
        assert!(requests.is_some());
    }

    #[test]
    fn test_parse_kind_9735_missing_required_tags() {
        let keys = Keys::generate();

        let tags = vec![
            Tag::parse(vec!["p".to_string(), "recipient_pubkey".to_string()]).unwrap(),
            // Missing bolt11 and description
        ];

        let event = EventBuilder::new(Kind::ZapReceipt, "", tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let result = parser.parse_kind_9735(&event);

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("missing required tags"));
    }

    #[test]
    fn test_parse_kind_9735_invalid_description() {
        let keys = Keys::generate();
        let recipient_keys = Keys::generate();
        let bolt11 = "lnbc1000n1...";
        let invalid_description = "not valid json";

        let tags = vec![
            Tag::parse(vec![
                "p".to_string(),
                recipient_keys.public_key().to_string(),
            ])
            .unwrap(),
            Tag::parse(vec!["bolt11".to_string(), bolt11.to_string()]).unwrap(),
            Tag::parse(vec![
                "description".to_string(),
                invalid_description.to_string(),
            ])
            .unwrap(),
        ];

        let event = EventBuilder::new(Kind::ZapReceipt, "", tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let result = parser.parse_kind_9735(&event);

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("failed to parse zap request"));
    }

    #[test]
    fn test_parse_wrong_kind() {
        let keys = Keys::generate();

        let event = EventBuilder::new(Kind::TextNote, "test", Vec::new())
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let result = parser.parse_kind_9735(&event);

        assert!(result.is_err());
    }

    #[test]
    fn test_parse_kind_9735_validation_failure() {
        let keys = Keys::generate();
        let recipient_keys = Keys::generate();
        let different_recipient = Keys::generate();
        let bolt11 = "lnbc1000n1...";

        // Create zap request with different recipient than the receipt
        let zap_request = ZapRequest {
            kind: 9734,
            pubkey: keys.public_key().to_hex(),
            content: "Great post!".to_string(),
            tags: vec![
                vec!["p".to_string(), different_recipient.public_key().to_hex()], // Different recipient
            ],
            signature: Some("mock_signature".to_string()),
        };

        let description = serde_json::to_string(&zap_request).unwrap();

        let tags = vec![
            Tag::parse(vec![
                "p".to_string(),
                recipient_keys.public_key().to_string(),
            ])
            .unwrap(), // Different from zap request
            Tag::parse(vec!["bolt11".to_string(), bolt11.to_string()]).unwrap(),
            Tag::parse(vec!["description".to_string(), description]).unwrap(),
        ];

        let event = EventBuilder::new(Kind::ZapReceipt, "", tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, _) = parser.parse_kind_9735(&event).unwrap();

        assert!(!parsed.valid); // Should be invalid due to recipient mismatch
    }
}
