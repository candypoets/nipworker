use crate::parser::{Parser, ParserError, Result};
use shared::{
    generated::nostr::*,
    types::{network::Request, Event},
}; // brings `fb::...` into scope

/// Parsed representation for NIP-23 (kind 30023) long-form content.
pub struct Kind30023Parsed {
    /// Unique slug (PRE "d" tag)
    pub slug: Option<String>,
    /// Human-readable title ("title" tag)
    pub title: Option<String>,
    /// Short summary ("summary" tag)
    pub summary: Option<String>,
    /// Cover image URL ("image" tag)
    pub image: Option<String>,
    /// Canonical URL ("canonical" tag)
    pub canonical: Option<String>,
    /// Topics/tags ("t" tags)
    pub topics: Vec<String>,
    /// Published timestamp in seconds ("published_at" tag)
    pub published_at: Option<u64>,
    /// PRE address string in "a" tuple form: "30023:<author_pubkey_hex>:<d>"
    pub naddr: Option<String>,
    /// Markdown content/body of the article
    pub content: String,
}

impl Parser {
    /// Parse a kind 30023 event extracting only explicit metadata from tags.
    ///
    /// - Does not parse markdown content.
    /// - Does not extract mentions/quotes.
    /// - Does not schedule follow-up network requests.
    pub fn parse_kind_30023(
        &self,
        event: &Event,
    ) -> Result<(Kind30023Parsed, Option<Vec<Request>>)> {
        if event.kind != 30023 {
            return Err(ParserError::Other("event is not kind 30023".to_string()));
        }

        // Read metadata from tags
        let slug = tag_value(&event.tags, "d");
        let title = tag_value(&event.tags, "title");
        let summary = tag_value(&event.tags, "summary");
        let image = tag_value(&event.tags, "image");
        let canonical = tag_value(&event.tags, "canonical");
        let topics = tag_values(&event.tags, "t");
        let published_at =
            tag_value(&event.tags, "published_at").and_then(|s| s.parse::<u64>().ok());

        // PRE "a" tuple string for convenience (not bech32 encoding)
        let naddr = slug
            .as_ref()
            .map(|d| format!("30023:{}:{}", event.pubkey.to_hex(), d));

        let parsed = Kind30023Parsed {
            slug,
            title,
            summary,
            image,
            canonical,
            topics,
            published_at,
            naddr,
            content: event.content.clone(),
        };

        // No additional network requests needed for long-form parsing
        Ok((parsed, None))
    }
}

/// Build the FlatBuffer for `Kind30023Parsed`.
pub fn build_flatbuffer<'a, A: flatbuffers::Allocator + 'a>(
    parsed: &Kind30023Parsed,
    builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
) -> Result<flatbuffers::WIPOffset<fb::Kind30023Parsed<'a>>> {
    // Optional strings
    let slug = parsed.slug.as_ref().map(|s| builder.create_string(s));
    let title = parsed.title.as_ref().map(|s| builder.create_string(s));
    let summary = parsed.summary.as_ref().map(|s| builder.create_string(s));
    let image = parsed.image.as_ref().map(|s| builder.create_string(s));
    let canonical = parsed.canonical.as_ref().map(|s| builder.create_string(s));
    let naddr = parsed.naddr.as_ref().map(|s| builder.create_string(s));
    let content = builder.create_string(&parsed.content);

    // Topics vector
    let topic_offsets: Vec<_> = parsed
        .topics
        .iter()
        .map(|t| builder.create_string(t))
        .collect();
    let topics = if topic_offsets.is_empty() {
        None
    } else {
        Some(builder.create_vector(&topic_offsets))
    };

    let args = fb::Kind30023ParsedArgs {
        slug,
        title,
        summary,
        image,
        canonical,
        topics,
        // FlatBuffers scalars are not Option; use a default of 0 when absent
        published_at: parsed.published_at.unwrap_or(0),
        naddr,
        content: Some(content),
    };

    let offset = fb::Kind30023Parsed::create(builder, &args);
    Ok(offset)
}

/// Utility: get the first value for a tag key, e.g., ["title", "<value>"]
fn tag_value(tags: &[Vec<String>], key: &str) -> Option<String> {
    tags.iter()
        .find_map(|t| (t.len() >= 2 && t[0] == key).then(|| t[1].clone()))
}

/// Utility: get all values for a tag key that can repeat, e.g., ["t", "<topic>"]
fn tag_values(tags: &[Vec<String>], key: &str) -> Vec<String> {
    tags.iter()
        .filter_map(|t| (t.len() >= 2 && t[0] == key).then(|| t[1].clone()))
        .collect()
}
