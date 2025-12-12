use crate::parser::Parser;
use crate::parser::{ParserError, Result};
use crate::types::network::Request;
use crate::types::nostr::Event;
use crate::utils::request_deduplication::RequestDeduplicator;

// NEW: Imports for FlatBuffers
use shared::generated::nostr::*;

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
    // Helper function to find the last tag with a specific name
    pub fn find_last_tag<'a>(tags: &'a [Vec<String>], tag_name: &str) -> Option<&'a Vec<String>> {
        tags.iter()
            .rev()
            .find(|tag| !tag.is_empty() && tag[0] == tag_name)
    }

    pub fn parse_kind_7(&self, event: &Event) -> Result<(Kind7Parsed, Option<Vec<Request>>)> {
        if event.kind != 7 {
            return Err(ParserError::Other("event is not kind 7".to_string()));
        }

        let mut requests = Vec::new();

        // Request profile information for the author
        requests.push(Request {
            authors: vec![event.pubkey.to_hex()],
            kinds: vec![0],
            relays: vec![],
            close_on_eose: true,
            cache_first: true,
            ..Default::default()
        });

        // Find the e tag for the target event (should be the last one if multiple)
        let e_tag = Self::find_last_tag(&event.tags, "e").ok_or_else(|| {
            ParserError::Other("reaction must have at least one e tag".to_string())
        })?;

        if e_tag.len() < 2 {
            return Err(ParserError::Other("invalid e tag format".to_string()));
        }

        let event_id = e_tag[1].clone();

        // Find pubkey tag (last p tag)
        let pubkey = Self::find_last_tag(&event.tags, "p")
            .and_then(|tag| {
                if tag.len() >= 2 {
                    Some(tag[1].clone())
                } else {
                    None
                }
            })
            .unwrap_or_default();

        // Find kind tag
        let event_kind = Self::find_last_tag(&event.tags, "k").and_then(|tag| {
            if tag.len() >= 2 {
                tag[1].parse::<u64>().ok()
            } else {
                None
            }
        });

        // Find addressable coordinates
        let target_coordinates = Self::find_last_tag(&event.tags, "a").and_then(|tag| {
            if tag.len() >= 2 {
                Some(tag[1].clone())
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
        let deduplicated_requests = RequestDeduplicator::deduplicate_requests(&requests);

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
            if tag.len() >= 3 && tag[0] == "emoji" && tag[1] == shortcode {
                return Some(Emoji {
                    shortcode: shortcode.to_string(),
                    url: tag[2].clone(),
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
