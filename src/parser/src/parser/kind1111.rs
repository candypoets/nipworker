use crate::parser::{
    content::{parse_content, ContentBlock},
    Parser, ParserError, Result,
};
use shared::{
    generated::nostr::*,
    types::{network::Request, Event},
};

/// Event pointer for citations (q tags or nostr: refs)
pub struct EventPointer {
    pub id: String,
    pub relays: Vec<String>,
    pub author: Option<String>,
    pub kind: Option<u64>,
}

/// Profile pointer for mentions
pub struct ProfilePointer {
    pub pubkey: String,
    pub relays: Vec<String>,
}

/// Parsed representation for NIP-22 (kind 1111) comments.
pub struct Kind1111Parsed {
    /// Raw content text (plaintext)
    pub content: String,
    /// Parsed content blocks (mentions, URLs, hashtags, nostr: refs)
    pub parsed_content: Vec<ContentBlock>,

    // Root scope (uppercase tags)
    pub root_id: Option<String>,
    pub root_coordinate: Option<String>,
    pub root_external: Option<String>,
    pub root_kind: Option<u16>,
    pub root_author: Option<String>,
    pub root_relays: Vec<String>,

    // Parent scope (lowercase tags)
    pub parent_id: Option<String>,
    pub parent_coordinate: Option<String>,
    pub parent_external: Option<String>,
    pub parent_kind: Option<u16>,
    pub parent_author: Option<String>,
    pub parent_relays: Vec<String>,

    // Citations and mentions
    pub citations: Vec<EventPointer>,
    pub mentions: Vec<ProfilePointer>,
}

impl Parser {
    /// Parse a kind 1111 comment event.
    pub fn parse_kind_1111(&self, event: &Event) -> Result<(Kind1111Parsed, Option<Vec<Request>>)> {
        if event.kind != 1111 {
            return Err(ParserError::Other("event is not kind 1111".to_string()));
        }

        let mut requests = Vec::new();

        // Parse content for mentions, URLs, hashtags, nostr: refs
        let parsed_content = parse_content(&event.content)?;

        // Extract event mentions from parsed content (nostr: refs)
        let content_mentions = extract_content_mentions(&parsed_content);

        // Extract root scope (uppercase tags: E, A, I, K, P)
        let root_id = tag_value(&event.tags, "E");
        let root_coordinate = tag_value(&event.tags, "A");
        let root_external = tag_value(&event.tags, "I");
        let root_kind = tag_value(&event.tags, "K").and_then(|s| s.parse::<u16>().ok());
        let (root_author, root_relays) = tag_value_with_relay(&event.tags, "P");

        // Extract parent scope (lowercase tags: e, a, i, k, p)
        let parent_id = tag_value(&event.tags, "e");
        let parent_coordinate = tag_value(&event.tags, "a");
        let parent_external = tag_value(&event.tags, "i");
        let parent_kind = tag_value(&event.tags, "k").and_then(|s| s.parse::<u16>().ok());
        let (parent_author, parent_relays) = tag_value_with_relay(&event.tags, "p");

        // Extract citations (q tags)
        let citations = extract_citations(&event.tags);

        // Add requests for cited events
        for citation in &citations {
            if !citation.relays.is_empty() {
                requests.push(Request {
                    ids: vec![citation.id.clone()],
                    relays: citation.relays.clone(),
                    close_on_eose: true,
                    cache_first: true,
                    ..Default::default()
                });
            }
        }

        // Combine p tags from tags and parsed content for mentions
        let tag_mentions = extract_p_tag_mentions(&event.tags);
        let mut all_mentions = tag_mentions;
        all_mentions.extend(content_mentions);

        // Deduplicate mentions by pubkey
        let mut seen = std::collections::HashSet::new();
        all_mentions.retain(|m| seen.insert(m.pubkey.clone()));

        let parsed = Kind1111Parsed {
            content: event.content.clone(),
            parsed_content,
            root_id,
            root_coordinate,
            root_external,
            root_kind,
            root_author,
            root_relays,
            parent_id,
            parent_coordinate,
            parent_external,
            parent_kind,
            parent_author,
            parent_relays,
            citations,
            mentions: all_mentions,
        };

        let requests = if requests.is_empty() {
            None
        } else {
            Some(requests)
        };

        Ok((parsed, requests))
    }
}

/// Extract mentions from parsed content blocks (nostr: refs).
fn extract_content_mentions(blocks: &[ContentBlock]) -> Vec<ProfilePointer> {
    let mut mentions = Vec::new();

    for block in blocks {
        if let Some(crate::parser::content::ContentData::Nostr { id, entity, author, .. }) =
            block.data.as_ref()
        {
            // Only include profile mentions (nprofile, npub)
            if entity == "nprofile" || entity == "npub" {
                mentions.push(ProfilePointer {
                    pubkey: id.clone(),
                    relays: Vec::new(),
                });
            }
        }
    }

    mentions
}

/// Extract q-tag citations.
fn extract_citations(tags: &[Vec<String>]) -> Vec<EventPointer> {
    tags.iter()
        .filter_map(|tag| {
            if tag.len() < 2 || tag[0] != "q" {
                return None;
            }

            let id = tag[1].clone();
            let relays = if tag.len() >= 3 && !tag[2].is_empty() {
                vec![tag[2].clone()]
            } else {
                Vec::new()
            };
            let author = if tag.len() >= 4 {
                Some(tag[3].clone())
            } else {
                None
            };

            Some(EventPointer {
                id,
                relays,
                author,
                kind: None,
            })
        })
        .collect()
}

/// Extract p-tag mentions.
fn extract_p_tag_mentions(tags: &[Vec<String>]) -> Vec<ProfilePointer> {
    tags.iter()
        .filter_map(|tag| {
            if tag.len() < 2 || tag[0] != "p" {
                return None;
            }

            let pubkey = tag[1].clone();
            let relays = if tag.len() >= 3 && !tag[2].is_empty() {
                vec![tag[2].clone()]
            } else {
                Vec::new()
            };

            Some(ProfilePointer { pubkey, relays })
        })
        .collect()
}

/// Build the FlatBuffer for `Kind1111Parsed`.
pub fn build_flatbuffer<'a, A: flatbuffers::Allocator + 'a>(
    parsed: &Kind1111Parsed,
    builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
) -> Result<flatbuffers::WIPOffset<fb::Kind1111Parsed<'a>>> {
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

    // Root scope strings
    let root_id = parsed.root_id.as_ref().map(|s| builder.create_string(s));
    let root_coordinate = parsed.root_coordinate.as_ref().map(|s| builder.create_string(s));
    let root_external = parsed.root_external.as_ref().map(|s| builder.create_string(s));
    let root_author = parsed.root_author.as_ref().map(|s| builder.create_string(s));
    let root_relays = if parsed.root_relays.is_empty() {
        None
    } else {
        let offsets: Vec<_> = parsed.root_relays.iter().map(|r| builder.create_string(r)).collect();
        Some(builder.create_vector(&offsets))
    };

    // Parent scope strings
    let parent_id = parsed.parent_id.as_ref().map(|s| builder.create_string(s));
    let parent_coordinate = parsed.parent_coordinate.as_ref().map(|s| builder.create_string(s));
    let parent_external = parsed.parent_external.as_ref().map(|s| builder.create_string(s));
    let parent_author = parsed.parent_author.as_ref().map(|s| builder.create_string(s));
    let parent_relays = if parsed.parent_relays.is_empty() {
        None
    } else {
        let offsets: Vec<_> = parsed.parent_relays.iter().map(|r| builder.create_string(r)).collect();
        Some(builder.create_vector(&offsets))
    };

    // Build citations vector
    let mut citation_offsets = Vec::new();
    for cit in &parsed.citations {
        let id = builder.create_string(&cit.id);
        let relay_offsets: Vec<_> = cit.relays.iter().map(|r| builder.create_string(r)).collect();
        let relays = if relay_offsets.is_empty() {
            None
        } else {
            Some(builder.create_vector(&relay_offsets))
        };
        let author = cit.author.as_ref().map(|a| builder.create_string(a));
        let args = fb::EventPointerArgs {
            id: Some(id),
            relays,
            author,
            kind: cit.kind.unwrap_or(0),
        };
        citation_offsets.push(fb::EventPointer::create(builder, &args));
    }
    let citations = if citation_offsets.is_empty() {
        None
    } else {
        Some(builder.create_vector(&citation_offsets))
    };

    // Build mentions vector
    let mut mention_offsets = Vec::new();
    for mention in &parsed.mentions {
        let pubkey = builder.create_string(&mention.pubkey);
        let relay_offsets: Vec<_> = mention.relays.iter().map(|r| builder.create_string(r)).collect();
        let relays = if relay_offsets.is_empty() {
            None
        } else {
            Some(builder.create_vector(&relay_offsets))
        };
        let args = fb::ProfilePointerArgs {
            public_key: Some(pubkey),
            relays,
        };
        mention_offsets.push(fb::ProfilePointer::create(builder, &args));
    }
    let mentions = if mention_offsets.is_empty() {
        None
    } else {
        Some(builder.create_vector(&mention_offsets))
    };

    let args = fb::Kind1111ParsedArgs {
        content: Some(content),
        parsed_content,
        root_id,
        root_coordinate,
        root_external,
        root_kind: parsed.root_kind.unwrap_or(0),
        root_author,
        root_relays,
        parent_id,
        parent_coordinate,
        parent_external,
        parent_kind: parsed.parent_kind.unwrap_or(0),
        parent_author,
        parent_relays,
        citations,
        mentions,
    };

    let offset = fb::Kind1111Parsed::create(builder, &args);
    Ok(offset)
}

/// Utility: get the first value for a tag key.
fn tag_value(tags: &[Vec<String>], key: &str) -> Option<String> {
    tags.iter()
        .find_map(|t| (t.len() >= 2 && t[0] == key).then(|| t[1].clone()))
}

/// Utility: get value and relay hint for a tag key.
fn tag_value_with_relay(tags: &[Vec<String>], key: &str) -> (Option<String>, Vec<String>) {
    for tag in tags {
        if tag.len() >= 2 && tag[0] == key {
            let value = tag[1].clone();
            let relays = if tag.len() >= 3 && !tag[2].is_empty() {
                vec![tag[2].clone()]
            } else {
                Vec::new()
            };
            return (Some(value), relays);
        }
    }
    (None, Vec::new())
}
