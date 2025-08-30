use crate::parser::Parser;
use crate::types::network::Request;
use anyhow::{anyhow, Result};
use nostr::Event;
use serde::{Deserialize, Serialize};

// NEW: Imports for FlatBuffers
use crate::generated::nostr::*;
use flatbuffers::FlatBufferBuilder;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReactionType {
    #[serde(rename = "+")]
    Like,
    #[serde(rename = "-")]
    Dislike,
    #[serde(rename = "emoji")]
    Emoji,
    #[serde(rename = "custom")]
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Emoji {
    pub shortcode: String,
    pub url: String,
}

// Kind17Parsed reuses Kind7Parsed type as per the TypeScript implementation
pub type Kind17Parsed = crate::parser::kind7::Kind7Parsed;

impl Parser {
    pub fn parse_kind_17(&self, event: &Event) -> Result<(Kind17Parsed, Option<Vec<Request>>)> {
        if event.kind.as_u64() != 17 {
            return Err(anyhow!("event is not kind 17"));
        }

        // Find the r tag for the URL being reacted to
        let _r_tag = event
            .tags
            .iter()
            .find(|tag| {
                let tag_vec = tag.as_vec();
                tag_vec.len() >= 2 && tag_vec[0] == "r"
            })
            .ok_or_else(|| anyhow!("kind 17 must have an r tag"))?;

        // Parse reaction type
        let content = &event.content;
        let reaction_type = match content.as_str() {
            "+" => crate::parser::kind7::ReactionType::Like,
            "-" => crate::parser::kind7::ReactionType::Dislike,
            _ if content.starts_with(':') && content.ends_with(':') => {
                crate::parser::kind7::ReactionType::Emoji
            }
            _ => crate::parser::kind7::ReactionType::Custom,
        };

        // Parse emoji if present
        let emoji = if matches!(reaction_type, crate::parser::kind7::ReactionType::Emoji) {
            self.parse_emoji_content_kind17(event)
        } else {
            None
        };

        let result = crate::parser::kind7::Kind7Parsed {
            reaction_type,
            event_id: String::new(), // No event ID for website reactions
            pubkey: String::new(),   // No pubkey for website reactions
            event_kind: None,
            emoji,
            target_coordinates: None,
        };

        Ok((result, None))
    }

    fn parse_emoji_content_kind17(&self, event: &Event) -> Option<crate::parser::kind7::Emoji> {
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
                return Some(crate::parser::kind7::Emoji {
                    shortcode: shortcode.to_string(),
                    url: tag_vec[2].clone(),
                });
            }
        }

        None
    }
}

// NEW: Build the FlatBuffer for Kind17Parsed
pub fn build_flatbuffer<'a, A: flatbuffers::Allocator + 'a>(
    parsed: &Kind17Parsed,
    builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
) -> Result<flatbuffers::WIPOffset<fb::Kind17Parsed<'a>>> {
    let reaction_type = match parsed.reaction_type {
        crate::parser::kind7::ReactionType::Like => fb::ReactionType::Like,
        crate::parser::kind7::ReactionType::Dislike => fb::ReactionType::Dislike,
        crate::parser::kind7::ReactionType::Emoji => fb::ReactionType::Emoji,
        crate::parser::kind7::ReactionType::Custom => fb::ReactionType::Custom,
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

    let args = fb::Kind17ParsedArgs {
        reaction_type,
        event_id: Some(event_id),
        pubkey: Some(pubkey),
        event_kind: parsed.event_kind.unwrap_or(0),
        emoji,
        target_coordinates,
    };

    let offset = fb::Kind17Parsed::create(builder, &args);

    Ok(offset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind, Tag};

    #[test]
    fn test_parse_kind_17_basic() {
        let keys = Keys::generate();
        let website_url = "https://example.com";

        let tags = vec![Tag::parse(vec!["r".to_string(), website_url.to_string()]).unwrap()];

        let event = EventBuilder::new(Kind::Custom(17), "+", tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, _) = parser.parse_kind_17(&event).unwrap();

        assert!(matches!(
            parsed.reaction_type,
            crate::parser::kind7::ReactionType::Like
        ));
        assert_eq!(parsed.event_id, ""); // No event ID for website reactions
        assert_eq!(parsed.pubkey, ""); // No pubkey for website reactions
    }

    #[test]
    fn test_parse_kind_17_no_r_tag() {
        let keys = Keys::generate();

        let event = EventBuilder::new(Kind::Custom(17), "+", Vec::new())
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let result = parser.parse_kind_17(&event);

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("must have an r tag"));
    }

    #[test]
    fn test_parse_wrong_kind() {
        let keys = Keys::generate();

        let event = EventBuilder::new(Kind::TextNote, "test", Vec::new())
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let result = parser.parse_kind_17(&event);

        assert!(result.is_err());
    }
}
