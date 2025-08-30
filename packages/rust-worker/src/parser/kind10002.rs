use crate::parser::Parser;
use crate::types::network::Request;
use anyhow::{anyhow, Result};
use nostr::Event;
use serde::{Deserialize, Serialize};

// NEW: Imports for FlatBuffers
use crate::generated::nostr::*;
use flatbuffers::FlatBufferBuilder;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayInfo {
    pub url: String,
    pub read: bool,
    pub write: bool,
}

pub type Kind10002Parsed = Vec<RelayInfo>;

impl Parser {
    pub fn parse_kind_10002(
        &self,
        event: &Event,
    ) -> Result<(Kind10002Parsed, Option<Vec<Request>>)> {
        if event.kind.as_u64() != 10002 {
            return Err(anyhow!("event is not kind 10002"));
        }

        let mut relays = Vec::new();

        // Extract relay info from the r tags
        for tag in &event.tags {
            let tag_vec = tag.as_vec();
            if tag_vec.len() >= 2 && tag_vec[0] == "r" && !tag_vec[1].is_empty() {
                let url = normalize_relay_url(&tag_vec[1]);
                if url.is_empty() {
                    continue;
                }

                let marker = if tag_vec.len() >= 3 {
                    tag_vec[2].to_lowercase()
                } else {
                    String::new()
                };

                // If no marker is provided, the relay is used for both read and write
                // If a marker is provided, it should be either "read", "write", or both
                let relay = RelayInfo {
                    url: url.clone(),
                    read: marker.is_empty() || marker == "read",
                    write: marker.is_empty() || marker == "write",
                };

                relays.push(relay);
            }
        }

        // Deduplicate relays by URL
        let mut unique_relays = std::collections::HashMap::new();
        for relay in relays {
            unique_relays.insert(relay.url.clone(), relay);
        }

        // Convert map to vec
        let result: Kind10002Parsed = unique_relays.into_values().collect();

        Ok((result, None))
    }
}

// NEW: Build the FlatBuffer for Kind10002Parsed
pub fn build_flatbuffer<'a, A: flatbuffers::Allocator + 'a>(
    parsed: &Kind10002Parsed,
    builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
) -> Result<flatbuffers::WIPOffset<fb::Kind10002Parsed<'a>>> {
    // Build relay info vector
    let mut relay_info_offsets = Vec::new();
    for relay in parsed {
        let url = builder.create_string(&relay.url);

        let relay_info_args = fb::RelayInfoArgs {
            url: Some(url),
            read: relay.read,
            write: relay.write,
        };
        let relay_info_offset = fb::RelayInfo::create(builder, &relay_info_args);
        relay_info_offsets.push(relay_info_offset);
    }
    let relay_info_vector = builder.create_vector(&relay_info_offsets);

    let args = fb::Kind10002ParsedArgs {
        relays: Some(relay_info_vector),
    };

    let offset = fb::Kind10002Parsed::create(builder, &args);

    Ok(offset)
}

fn normalize_relay_url(url: &str) -> String {
    let url = url.trim();
    if url.is_empty() {
        return String::new();
    }

    // Basic URL normalization - could use nostr::Url::normalize if available
    if url.starts_with("wss://") || url.starts_with("ws://") {
        url.to_string()
    } else if url.starts_with("//") {
        format!("wss:{}", url)
    } else {
        format!("wss://{}", url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind, Tag};

    #[test]
    fn test_parse_kind_10002_basic() {
        let keys = Keys::generate();
        let relay_url = "wss://relay.example.com";

        let tags = vec![Tag::parse(vec!["r".to_string(), relay_url.to_string()]).unwrap()];

        let event = EventBuilder::new(Kind::RelayList, "", tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, _) = parser.parse_kind_10002(&event).unwrap();

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].url, relay_url);
        assert!(parsed[0].read);
        assert!(parsed[0].write);
    }

    #[test]
    fn test_parse_kind_10002_with_markers() {
        let keys = Keys::generate();
        let read_relay = "wss://read.example.com";
        let write_relay = "wss://write.example.com";

        let tags = vec![
            Tag::parse(vec![
                "r".to_string(),
                read_relay.to_string(),
                "read".to_string(),
            ])
            .unwrap(),
            Tag::parse(vec![
                "r".to_string(),
                write_relay.to_string(),
                "write".to_string(),
            ])
            .unwrap(),
        ];

        let event = EventBuilder::new(Kind::RelayList, "", tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, _) = parser.parse_kind_10002(&event).unwrap();

        assert_eq!(parsed.len(), 2);

        let read_info = parsed.iter().find(|r| r.url == read_relay).unwrap();
        assert!(read_info.read);
        assert!(!read_info.write);

        let write_info = parsed.iter().find(|r| r.url == write_relay).unwrap();
        assert!(!write_info.read);
        assert!(write_info.write);
    }

    #[test]
    fn test_parse_kind_10002_deduplication() {
        let keys = Keys::generate();
        let relay_url = "wss://relay.example.com";

        let tags = vec![
            Tag::parse(vec!["r".to_string(), relay_url.to_string()]).unwrap(),
            Tag::parse(vec!["r".to_string(), relay_url.to_string()]).unwrap(), // Duplicate
        ];

        let event = EventBuilder::new(Kind::RelayList, "", tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, _) = parser.parse_kind_10002(&event).unwrap();

        assert_eq!(parsed.len(), 1); // Should be deduplicated
    }

    #[test]
    fn test_parse_wrong_kind() {
        let keys = Keys::generate();

        let event = EventBuilder::new(Kind::TextNote, "test", Vec::new())
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let result = parser.parse_kind_10002(&event);

        assert!(result.is_err());
    }
}
