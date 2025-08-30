use crate::parser::Parser;
use crate::types::network::Request;
use anyhow::{anyhow, Result};
use nostr::Event;
use serde::{Deserialize, Serialize};

// NEW: Imports for FlatBuffers
use crate::generated::nostr::*;
use flatbuffers::FlatBufferBuilder;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contact {
    pub pubkey: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub relays: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub petname: Option<String>,
}

pub type Kind3Parsed = Vec<Contact>;

impl Parser {
    pub fn parse_kind_3(&self, event: &Event) -> Result<(Kind3Parsed, Option<Vec<Request>>)> {
        if event.kind.as_u64() != 3 {
            return Err(anyhow!("event is not kind 3"));
        }

        let mut contacts = Vec::new();

        // Extract contacts from p tags
        for tag in &event.tags {
            let tag_vec = tag.as_vec();
            if tag_vec.len() >= 2 && tag_vec[0] == "p" {
                let mut contact = Contact {
                    pubkey: tag_vec[1].clone(),
                    relays: Vec::new(),
                    petname: None,
                };

                // Add relay if present (position 2)
                if tag_vec.len() >= 3 && !tag_vec[2].is_empty() {
                    contact.relays = vec![tag_vec[2].clone()];
                }

                // Add petname if present (position 3)
                if tag_vec.len() >= 4 && !tag_vec[3].is_empty() {
                    contact.petname = Some(tag_vec[3].clone());
                }

                contacts.push(contact);
            }
        }

        Ok((contacts, None))
    }
}

// NEW: Build the FlatBuffer for Kind3Parsed
pub fn build_flatbuffer<'a, A: flatbuffers::Allocator + 'a>(
    parsed: &Kind3Parsed,
    builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
) -> Result<flatbuffers::WIPOffset<fb::Kind3Parsed<'a>>> {
    // Build contacts vector
    let mut contacts_offsets = Vec::new();
    for contact in parsed {
        let pubkey = builder.create_string(&contact.pubkey);
        let relays_offsets: Vec<_> = contact
            .relays
            .iter()
            .map(|r| builder.create_string(r))
            .collect();
        let relays = if relays_offsets.is_empty() {
            None
        } else {
            Some(builder.create_vector(&relays_offsets))
        };
        let petname = contact.petname.as_ref().map(|p| builder.create_string(p));

        let contact_args = fb::ContactArgs {
            pubkey: Some(pubkey),
            relays,
            petname,
        };
        let contact_offset = fb::Contact::create(builder, &contact_args);
        contacts_offsets.push(contact_offset);
    }
    let contacts_vector = builder.create_vector(&contacts_offsets);

    let args = fb::Kind3ParsedArgs {
        contacts: Some(contacts_vector),
    };

    let offset = fb::Kind3Parsed::create(builder, &args);

    Ok(offset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind, Tag};

    #[test]
    fn test_parse_kind_3_basic() {
        let keys = Keys::generate();
        let pubkey1 = "npub1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
        let pubkey2 = "npub2345678901bcdef2345678901bcdef2345678901bcdef2345678901bcdef2";

        let tags = vec![
            Tag::parse(vec!["p".to_string(), pubkey1.to_string()]).unwrap(),
            Tag::parse(vec!["p".to_string(), pubkey2.to_string()]).unwrap(),
        ];

        let event = EventBuilder::new(Kind::ContactList, "", tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, _) = parser.parse_kind_3(&event).unwrap();

        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].pubkey, pubkey1);
        assert_eq!(parsed[1].pubkey, pubkey2);
        assert!(parsed[0].relays.is_empty());
        assert!(parsed[0].petname.is_none());
    }

    #[test]
    fn test_parse_kind_3_with_relay_and_petname() {
        let keys = Keys::generate();
        let pubkey = "npub1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
        let relay = "wss://relay.example.com";
        let petname = "alice";

        let tags = vec![Tag::parse(vec![
            "p".to_string(),
            pubkey.to_string(),
            relay.to_string(),
            petname.to_string(),
        ])
        .unwrap()];

        let event = EventBuilder::new(Kind::ContactList, "", tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, _) = parser.parse_kind_3(&event).unwrap();

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].pubkey, pubkey);
        assert_eq!(parsed[0].relays, vec![relay]);
        assert_eq!(parsed[0].petname, Some(petname.to_string()));
    }

    #[test]
    fn test_parse_kind_3_with_empty_relay() {
        let keys = Keys::generate();
        let pubkey = "npub1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
        let petname = "bob";

        let tags = vec![Tag::parse(vec![
            "p".to_string(),
            pubkey.to_string(),
            "".to_string(),
            petname.to_string(),
        ])
        .unwrap()];

        let event = EventBuilder::new(Kind::ContactList, "", tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, _) = parser.parse_kind_3(&event).unwrap();

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].pubkey, pubkey);
        assert!(parsed[0].relays.is_empty()); // Empty relay should result in empty vec
        assert_eq!(parsed[0].petname, Some(petname.to_string()));
    }

    #[test]
    fn test_parse_kind_3_no_p_tags() {
        let keys = Keys::generate();

        let tags = vec![
            Tag::parse(vec!["t".to_string(), "hashtag".to_string()]).unwrap(),
            Tag::parse(vec!["r".to_string(), "wss://relay.example.com".to_string()]).unwrap(),
        ];

        let event = EventBuilder::new(Kind::ContactList, "", tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, _) = parser.parse_kind_3(&event).unwrap();

        assert_eq!(parsed.len(), 0);
    }

    #[test]
    fn test_parse_kind_3_mixed_tags() {
        let keys = Keys::generate();
        let pubkey1 = "npub1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
        let pubkey2 = "npub2345678901bcdef2345678901bcdef2345678901bcdef2345678901bcdef2";

        let tags = vec![
            Tag::parse(vec!["t".to_string(), "hashtag".to_string()]).unwrap(),
            Tag::parse(vec![
                "p".to_string(),
                pubkey1.to_string(),
                "wss://relay1.com".to_string(),
                "alice".to_string(),
            ])
            .unwrap(),
            Tag::parse(vec!["r".to_string(), "wss://relay.example.com".to_string()]).unwrap(),
            Tag::parse(vec!["p".to_string(), pubkey2.to_string()]).unwrap(),
        ];

        let event = EventBuilder::new(Kind::ContactList, "", tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, _) = parser.parse_kind_3(&event).unwrap();

        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].pubkey, pubkey1);
        assert_eq!(parsed[0].relays, vec!["wss://relay1.com"]);
        assert_eq!(parsed[0].petname, Some("alice".to_string()));
        assert_eq!(parsed[1].pubkey, pubkey2);
        assert!(parsed[1].relays.is_empty());
        assert!(parsed[1].petname.is_none());
    }

    #[test]
    fn test_parse_wrong_kind() {
        let keys = Keys::generate();

        let event = EventBuilder::new(Kind::TextNote, "test", Vec::new())
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let result = parser.parse_kind_3(&event);

        assert!(result.is_err());
    }
}
