use crate::parser::{
    content::{parse_content, ContentBlock},
    Parser, ParserError, Result,
};
use shared::{
    generated::nostr::*,
    types::{network::Request, Event},
};

/// Reference to a live activity (the "a" tag)
#[derive(Debug, Clone)]
pub struct LiveActivityRef {
    pub kind: u16,
    pub pubkey: String,
    pub identifier: String, // the "d" value
    pub relay: Option<String>,
}

/// Thread reference for reply threading (the "e" tag)
#[derive(Debug, Clone)]
pub struct LiveChatThreadRef {
    pub event_id: String,
    pub relay: Option<String>,
}

/// Participant mention (the "p" tag)
#[derive(Debug, Clone)]
pub struct LiveChatParticipant {
    pub pubkey: String,
    pub relay: Option<String>,
}

/// Parsed representation for NIP-53 kind 1311 live chat messages.
#[derive(Debug, Clone)]
pub struct Kind1311Parsed {
    /// Raw message content
    pub content: String,
    /// Parsed content blocks (mentions, URLs, hashtags, nostr: refs)
    pub parsed_content: Vec<ContentBlock>,
    /// The live activity being chatted about (required)
    pub activity: LiveActivityRef,
    /// Thread references (reply to other chat messages)
    pub thread_refs: Vec<LiveChatThreadRef>,
    /// Profile mentions
    pub mentions: Vec<LiveChatParticipant>,
}

impl Parser {
    /// Parse a kind 1311 live chat message event.
    pub fn parse_kind_1311(&self, event: &Event) -> Result<(Kind1311Parsed, Option<Vec<Request>>)> {
        if event.kind != 1311 {
            return Err(ParserError::Other("event is not kind 1311".to_string()));
        }

        // Parse content for mentions, URLs, hashtags, nostr: refs
        let parsed_content = parse_content(&event.content)?;

        // Extract the live activity reference (required "a" tag)
        let activity = extract_activity_ref(&event.tags)
            .ok_or_else(|| ParserError::Other("kind 1311 requires an 'a' tag referencing a live activity".to_string()))?;

        // Extract thread references ("e" tags)
        let thread_refs = extract_thread_refs(&event.tags);

        // Extract mentions ("p" tags)
        let mentions = extract_mentions(&event.tags);

        let parsed = Kind1311Parsed {
            content: event.content.clone(),
            parsed_content,
            activity,
            thread_refs,
            mentions,
        };

        Ok((parsed, None))
    }
}

/// Extract the live activity reference from "a" tags.
/// Expected format: ["a", "30311:<pubkey>:<d>", <relay?>]
fn extract_activity_ref(tags: &[Vec<String>]) -> Option<LiveActivityRef> {
    for tag in tags {
        if tag.len() < 2 || tag[0] != "a" {
            continue;
        }

        // Parse the coordinate: "30311:<pubkey>:<d>"
        let coord = &tag[1];
        let mut parts = coord.splitn(3, ':');
        let kind = parts.next()?.parse::<u16>().ok()?;
        
        // Must reference a live activity (30311)
        if kind != 30311 {
            continue;
        }

        let pubkey = parts.next()?.to_string();
        let identifier = parts.next()?.to_string();

        let relay = tag.get(2).cloned().filter(|s| !s.is_empty());

        return Some(LiveActivityRef {
            kind,
            pubkey,
            identifier,
            relay,
        });
    }
    None
}

/// Extract thread references from "e" tags.
/// Format: ["e", "<event_id>", <relay?>]
fn extract_thread_refs(tags: &[Vec<String>]) -> Vec<LiveChatThreadRef> {
    tags.iter()
        .filter_map(|tag| {
            if tag.len() < 2 || tag[0] != "e" {
                return None;
            }

            let event_id = tag[1].clone();
            let relay = tag.get(2).cloned().filter(|s| !s.is_empty());

            Some(LiveChatThreadRef { event_id, relay })
        })
        .collect()
}

/// Extract participant mentions from "p" tags.
/// Format: ["p", "<pubkey>", <relay?>]
fn extract_mentions(tags: &[Vec<String>]) -> Vec<LiveChatParticipant> {
    tags.iter()
        .filter_map(|tag| {
            if tag.len() < 2 || tag[0] != "p" {
                return None;
            }

            let pubkey = tag[1].clone();
            let relay = tag.get(2).cloned().filter(|s| !s.is_empty());

            Some(LiveChatParticipant { pubkey, relay })
        })
        .collect()
}

/// Build the FlatBuffer for `Kind1311Parsed`.
pub fn build_flatbuffer<'a, A: flatbuffers::Allocator + 'a>(
    parsed: &Kind1311Parsed,
    builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
) -> Result<flatbuffers::WIPOffset<fb::Kind1311Parsed<'a>>> {
    // Content strings
    let content = builder.create_string(&parsed.content);

    // Build parsed_content vector (ContentBlock)
    let mut content_block_offsets = Vec::new();
    for block in &parsed.parsed_content {
        let block_type = builder.create_string(&block.block_type);
        let text = builder.create_string(&block.text);

        // Build ContentData if present
        let (data_type, data) = match &block.data {
            Some(d) => crate::parser::content::serialize_content_data(builder, d),
            None => (fb::ContentData::NONE, None),
        };

        let args = fb::ContentBlockArgs {
            type_: Some(block_type),
            text: Some(text),
            data_type,
            data,
        };
        content_block_offsets.push(fb::ContentBlock::create(builder, &args));
    }
    let parsed_content = if content_block_offsets.is_empty() {
        None
    } else {
        Some(builder.create_vector(&content_block_offsets))
    };

    // Build activity reference
    let activity_pubkey = builder.create_string(&parsed.activity.pubkey);
    let activity_identifier = builder.create_string(&parsed.activity.identifier);
    let activity_relay = parsed.activity.relay.as_ref().map(|s| builder.create_string(s));

    let activity_args = fb::LiveActivityRefArgs {
        kind: parsed.activity.kind,
        pubkey: Some(activity_pubkey),
        identifier: Some(activity_identifier),
        relay: activity_relay,
    };
    let activity = fb::LiveActivityRef::create(builder, &activity_args);

    // Build thread_refs vector
    let mut thread_ref_offsets = Vec::new();
    for thread_ref in &parsed.thread_refs {
        let event_id = builder.create_string(&thread_ref.event_id);
        let relay = thread_ref.relay.as_ref().map(|s| builder.create_string(s));
        let args = fb::LiveChatThreadRefArgs {
            event_id: Some(event_id),
            relay: relay,
        };
        thread_ref_offsets.push(fb::LiveChatThreadRef::create(builder, &args));
    }
    let thread_refs = if thread_ref_offsets.is_empty() {
        None
    } else {
        Some(builder.create_vector(&thread_ref_offsets))
    };

    // Build mentions vector
    let mut mention_offsets = Vec::new();
    for mention in &parsed.mentions {
        let pubkey = builder.create_string(&mention.pubkey);
        let relay = mention.relay.as_ref().map(|s| builder.create_string(s));
        let args = fb::LiveChatParticipantArgs {
            pubkey: Some(pubkey),
            relay: relay,
        };
        mention_offsets.push(fb::LiveChatParticipant::create(builder, &args));
    }
    let mentions = if mention_offsets.is_empty() {
        None
    } else {
        Some(builder.create_vector(&mention_offsets))
    };

    let args = fb::Kind1311ParsedArgs {
        content: Some(content),
        parsed_content,
        activity: Some(activity),
        thread_refs,
        mentions,
    };

    let offset = fb::Kind1311Parsed::create(builder, &args);
    Ok(offset)
}
