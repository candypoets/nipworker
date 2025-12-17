use crate::parser::Parser;
use crate::parser::{ParserError, Result};
use shared::types::network::Request;
use shared::types::nostr::Event;

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

// Kind17Parsed reuses Kind7Parsed type as per the TypeScript implementation
pub type Kind17Parsed = crate::parser::kind7::Kind7Parsed;

impl Parser {
    pub fn parse_kind_17(&self, event: &Event) -> Result<(Kind17Parsed, Option<Vec<Request>>)> {
        if event.kind != 17 {
            return Err(ParserError::Other("event is not kind 17".to_string()));
        }

        // Find the r tag for the URL being reacted to
        let _r_tag = event
            .tags
            .iter()
            .find(|tag| tag.len() >= 2 && tag[0] == "r")
            .ok_or_else(|| ParserError::Other("kind 17 must have an r tag".to_string()))?;

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
            if tag.len() >= 3 && tag[0] == "emoji" && tag[1] == shortcode {
                return Some(crate::parser::kind7::Emoji {
                    shortcode: shortcode.to_string(),
                    url: tag[2].clone(),
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
