use crate::parser::{find_last_tag, Parser};
use crate::types::network::Request;
use crate::utils::request_deduplication::RequestDeduplicator;
use anyhow::{anyhow, Result};
use nostr::Event;

// NEW: Imports for FlatBuffers
use crate::generated::nostr::*;
use flatbuffers::FlatBufferBuilder;

pub enum ReactionType {
    Like,
    Dislike,
    Emoji,
    Custom,
}

pub struct Emoji {
    pub shortcode: String,
    pub url: String,
}

pub struct Kind7Parsed {
    pub reaction_type: ReactionType,
    pub event_id: String,
    pub pubkey: String,
    pub event_kind: Option<u64>,
    pub emoji: Option<Emoji>,
    pub target_coordinates: Option<String>,
}

impl Parser {
    pub fn parse_kind_7(&self, event: &Event) -> Result<(Kind7Parsed, Option<Vec<Request>>)> {
        if event.kind.as_u64() != 7 {
            return Err(anyhow!("event is not kind 7"));
        }

        let mut requests = Vec::new();

        // Request profile information for the author
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

        // Find the e tag for the target event (should be the last one if multiple)
        let e_tag = find_last_tag(&event.tags, "e")
            .ok_or_else(|| anyhow!("reaction must have at least one e tag"))?;

        let tag_vec = e_tag.as_vec();
        if tag_vec.len() < 2 {
            return Err(anyhow!("invalid e tag format"));
        }

        let event_id = tag_vec[1].clone();

        // Find pubkey tag (last p tag)
        let pubkey = find_last_tag(&event.tags, "p")
            .and_then(|tag| {
                let tag_vec = tag.as_vec();
                if tag_vec.len() >= 2 {
                    Some(tag_vec[1].clone())
                } else {
                    None
                }
            })
            .unwrap_or_default();

        // Find kind tag
        let event_kind = find_last_tag(&event.tags, "k").and_then(|tag| {
            let tag_vec = tag.as_vec();
            if tag_vec.len() >= 2 {
                tag_vec[1].parse::<u64>().ok()
            } else {
                None
            }
        });

        // Find addressable coordinates
        let target_coordinates = find_last_tag(&event.tags, "a").and_then(|tag| {
            let tag_vec = tag.as_vec();
            if tag_vec.len() >= 2 {
                Some(tag_vec[1].clone())
            } else {
                None
            }
        });

        // Parse reaction type
        let content = &event.content;
        let reaction_type = match content.as_str() {
            "+" | "" => ReactionType::Like,
            "-" => ReactionType::Dislike,
            _ if content.starts_with(':') && content.ends_with(':') => ReactionType::Emoji,
            _ => ReactionType::Custom,
        };

        // Parse emoji if present
        let emoji = if matches!(reaction_type, ReactionType::Emoji) {
            self.parse_emoji_content(event)
        } else {
            None
        };

        let result = Kind7Parsed {
            reaction_type,
            event_id,
            pubkey,
            event_kind,
            emoji,
            target_coordinates,
        };

        // Deduplicate requests using the utility
        let deduplicated_requests = RequestDeduplicator::deduplicate_requests(requests);

        Ok((result, Some(deduplicated_requests)))
    }

    fn parse_emoji_content(&self, event: &Event) -> Option<Emoji> {
        let content = &event.content;

        // Check if content is a shortcode format :emoji:
        if !content.starts_with(':') || !content.ends_with(':') {
            return None;
        }

        // Extract shortcode (remove the colons)
        let shortcode = &content[1..content.len() - 1];
        if shortcode.is_empty() {
            return None;
        }

        // Find matching emoji tag
        for tag in &event.tags {
            let tag_vec = tag.as_vec();
            if tag_vec.len() >= 3 && tag_vec[0] == "emoji" && tag_vec[1] == shortcode {
                return Some(Emoji {
                    shortcode: shortcode.to_string(),
                    url: tag_vec[2].clone(),
                });
            }
        }

        None
    }
}

// NEW: Build the FlatBuffer for Kind7Parsed
pub fn build_flatbuffer<'a, A: flatbuffers::Allocator + 'a>(
    parsed: &Kind7Parsed,
    builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
) -> Result<flatbuffers::WIPOffset<fb::Kind7Parsed<'a>>> {
    let reaction_type = match parsed.reaction_type {
        ReactionType::Like => fb::ReactionType::Like,
        ReactionType::Dislike => fb::ReactionType::Dislike,
        ReactionType::Emoji => fb::ReactionType::Emoji,
        ReactionType::Custom => fb::ReactionType::Custom,
    };

    let event_id = builder.create_string(&parsed.event_id);
    let pubkey = builder.create_string(&parsed.pubkey);
    let target_coordinates = parsed
        .target_coordinates
        .as_ref()
        .map(|s| builder.create_string(s));

    // Build emoji if present
    let emoji = parsed.emoji.as_ref().map(|e| {
        let shortcode = builder.create_string(&e.shortcode);
        let url = builder.create_string(&e.url);

        let emoji_args = fb::EmojiArgs {
            shortcode: Some(shortcode),
            url: Some(url),
        };
        fb::Emoji::create(builder, &emoji_args)
    });

    let args = fb::Kind7ParsedArgs {
        reaction_type,
        event_id: Some(event_id),
        pubkey: Some(pubkey),
        event_kind: parsed.event_kind.unwrap_or(0),
        emoji,
        target_coordinates,
    };

    let offset = fb::Kind7Parsed::create(builder, &args);

    Ok(offset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind, Tag};

    #[test]
    fn test_parse_kind_7_like() {
        let keys = Keys::generate();
        let target_event_id = "1234567890abcdef1234567890abcdef12345678";
        let target_pubkey = "abcdef1234567890abcdef1234567890abcdef12";

        let tags = vec![
            Tag::parse(vec!["e".to_string(), target_event_id.to_string()]).unwrap(),
            Tag::parse(vec!["p".to_string(), target_pubkey.to_string()]).unwrap(),
        ];

        let event = EventBuilder::new(Kind::Reaction, "+", tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, requests) = parser.parse_kind_7(&event).unwrap();

        assert!(matches!(parsed.reaction_type, ReactionType::Like));
        assert_eq!(parsed.event_id, target_event_id);
        assert_eq!(parsed.pubkey, target_pubkey);
        assert!(parsed.emoji.is_none());
        assert!(requests.is_some());
    }

    #[test]
    fn test_parse_kind_7_dislike() {
        let keys = Keys::generate();
        let target_event_id = "1234567890abcdef1234567890abcdef12345678";

        let tags = vec![Tag::parse(vec!["e".to_string(), target_event_id.to_string()]).unwrap()];

        let event = EventBuilder::new(Kind::Reaction, "-", tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, _) = parser.parse_kind_7(&event).unwrap();

        assert!(matches!(parsed.reaction_type, ReactionType::Dislike));
        assert_eq!(parsed.event_id, target_event_id);
    }

    #[test]
    fn test_parse_kind_7_emoji() {
        let keys = Keys::generate();
        let target_event_id = "1234567890abcdef1234567890abcdef12345678";
        let emoji_shortcode = "heart";
        let emoji_url = "https://example.com/heart.png";

        let tags = vec![
            Tag::parse(vec!["e".to_string(), target_event_id.to_string()]).unwrap(),
            Tag::parse(vec![
                "emoji".to_string(),
                emoji_shortcode.to_string(),
                emoji_url.to_string(),
            ])
            .unwrap(),
        ];

        let event = EventBuilder::new(Kind::Reaction, ":heart:", tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, _) = parser.parse_kind_7(&event).unwrap();

        assert!(matches!(parsed.reaction_type, ReactionType::Emoji));
        assert_eq!(parsed.event_id, target_event_id);
        assert!(parsed.emoji.is_some());

        let emoji = parsed.emoji.unwrap();
        assert_eq!(emoji.shortcode, emoji_shortcode);
        assert_eq!(emoji.url, emoji_url);
    }

    #[test]
    fn test_parse_kind_7_custom() {
        let keys = Keys::generate();
        let target_event_id = "1234567890abcdef1234567890abcdef12345678";

        let tags = vec![Tag::parse(vec!["e".to_string(), target_event_id.to_string()]).unwrap()];

        let event = EventBuilder::new(Kind::Reaction, "amazing!", tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, _) = parser.parse_kind_7(&event).unwrap();

        assert!(matches!(parsed.reaction_type, ReactionType::Custom));
        assert_eq!(parsed.event_id, target_event_id);
    }

    #[test]
    fn test_parse_kind_7_with_kind_tag() {
        let keys = Keys::generate();
        let target_event_id = "1234567890abcdef1234567890abcdef12345678";
        let target_kind = 1u64;

        let tags = vec![
            Tag::parse(vec!["e".to_string(), target_event_id.to_string()]).unwrap(),
            Tag::parse(vec!["k".to_string(), target_kind.to_string()]).unwrap(),
        ];

        let event = EventBuilder::new(Kind::Reaction, "+", tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, _) = parser.parse_kind_7(&event).unwrap();

        assert_eq!(parsed.event_kind, Some(target_kind));
    }

    #[test]
    fn test_parse_kind_7_with_addressable_event() {
        let keys = Keys::generate();
        let target_event_id = "1234567890abcdef1234567890abcdef12345678";
        let coordinates = "30023:pubkey:identifier";

        let tags = vec![
            Tag::parse(vec!["e".to_string(), target_event_id.to_string()]).unwrap(),
            Tag::parse(vec!["a".to_string(), coordinates.to_string()]).unwrap(),
        ];

        let event = EventBuilder::new(Kind::Reaction, "+", tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, _) = parser.parse_kind_7(&event).unwrap();

        assert_eq!(parsed.target_coordinates, Some(coordinates.to_string()));
    }

    #[test]
    fn test_parse_kind_7_no_e_tag() {
        let keys = Keys::generate();

        let event = EventBuilder::new(Kind::Reaction, "+", Vec::new())
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let result = parser.parse_kind_7(&event);

        assert!(result.is_err());
    }

    #[test]
    fn test_parse_wrong_kind() {
        let keys = Keys::generate();

        let event = EventBuilder::new(Kind::TextNote, "test", Vec::new())
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let result = parser.parse_kind_7(&event);

        assert!(result.is_err());
    }

    #[test]
    fn test_parse_emoji_without_tag() {
        let keys = Keys::generate();
        let target_event_id = "1234567890abcdef1234567890abcdef12345678";

        let tags = vec![Tag::parse(vec!["e".to_string(), target_event_id.to_string()]).unwrap()];

        let event = EventBuilder::new(Kind::Reaction, ":unknown:", tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, _) = parser.parse_kind_7(&event).unwrap();

        assert!(matches!(parsed.reaction_type, ReactionType::Emoji));
        assert!(parsed.emoji.is_none()); // No matching emoji tag
    }
}
