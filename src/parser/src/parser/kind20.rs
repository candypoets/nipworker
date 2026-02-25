use crate::parser::{Parser, ParserError, Result};
use shared::{
    generated::nostr::*,
    types::{network::Request, Event},
};

/// ImetaData for a single image/media item
pub struct ImetaData {
    pub url: String,
    pub mime_type: Option<String>,
    pub dim: Option<String>,
    pub alt: Option<String>,
    pub blurhash: Option<String>,
    pub hash: Option<String>,
    pub fallback: Vec<String>,
    pub annotate_user: Option<String>,
}

/// Parsed representation for NIP-68 (kind 20) picture-first events.
pub struct Kind20Parsed {
    /// Title from "title" tag
    pub title: Option<String>,
    /// Description/content text
    pub description: String,
    /// Images from "imeta" tags
    pub images: Vec<ImetaData>,
    /// Content warning from "content-warning" tag
    pub content_warning: Option<String>,
    /// Location from "location" tag
    pub location: Option<String>,
    /// Geohash from "g" tag
    pub geohash: Option<String>,
    /// Hashtags from "t" tags
    pub hashtags: Vec<String>,
    /// Tagged users from "p" tags
    pub mentions: Vec<ProfilePointer>,
}

impl Parser {
    /// Parse a kind 20 picture event.
    pub fn parse_kind_20(&self, event: &Event) -> Result<(Kind20Parsed, Option<Vec<Request>>)> {
        if event.kind != 20 {
            return Err(ParserError::Other("event is not kind 20".to_string()));
        }

        // Extract metadata from tags
        let title = tag_value(&event.tags, "title");
        let content_warning = tag_value(&event.tags, "content-warning");
        let location = tag_value(&event.tags, "location");
        let geohash = tag_value(&event.tags, "g");
        let hashtags = tag_values(&event.tags, "t");

        // Extract imeta tags for images
        let images = extract_imeta_tags(&event.tags);

        // Extract p-tag mentions
        let mentions = extract_mentions(&event.tags);

        let parsed = Kind20Parsed {
            title,
            description: event.content.clone(),
            images,
            content_warning,
            location,
            geohash,
            hashtags,
            mentions,
        };

        Ok((parsed, None))
    }
}

/// Extract imeta tags from event tags.
fn extract_imeta_tags(tags: &[Vec<String>]) -> Vec<ImetaData> {
    let mut images = Vec::new();

    for tag in tags {
        if tag.len() < 2 || tag[0] != "imeta" {
            continue;
        }

        let mut url = None;
        let mut mime_type = None;
        let mut dim = None;
        let mut alt = None;
        let mut blurhash = None;
        let mut hash = None;
        let mut fallback = Vec::new();
        let mut annotate_user = None;

        // Parse each field in the imeta tag (skip tag[0] which is "imeta")
        for field in &tag[1..] {
            if let Some((key, value)) = field.split_once(' ') {
                let value = value.trim();
                match key {
                    "url" => url = Some(value.to_string()),
                    "m" => mime_type = Some(value.to_string()),
                    "dim" => dim = Some(value.to_string()),
                    "alt" => alt = Some(value.to_string()),
                    "blurhash" => blurhash = Some(value.to_string()),
                    "x" => hash = Some(value.to_string()),
                    "fallback" => fallback.push(value.to_string()),
                    "annotate-user" => annotate_user = Some(value.to_string()),
                    _ => {} // Ignore unknown fields
                }
            }
        }

        // Only add if we have a URL
        if let Some(url) = url {
            images.push(ImetaData {
                url,
                mime_type,
                dim,
                alt,
                blurhash,
                hash,
                fallback,
                annotate_user,
            });
        }
    }

    images
}

/// Extract p-tag mentions (profile pointers).
fn extract_mentions(tags: &[Vec<String>]) -> Vec<ProfilePointer> {
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

/// Build the FlatBuffer for `Kind20Parsed`.
pub fn build_flatbuffer<'a, A: flatbuffers::Allocator + 'a>(
    parsed: &Kind20Parsed,
    builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
) -> Result<flatbuffers::WIPOffset<fb::Kind20Parsed<'a>>> {
    // Optional strings
    let title = parsed.title.as_ref().map(|s| builder.create_string(s));
    let description = builder.create_string(&parsed.description);
    let content_warning = parsed
        .content_warning
        .as_ref()
        .map(|s| builder.create_string(s));
    let location = parsed.location.as_ref().map(|s| builder.create_string(s));
    let geohash = parsed.geohash.as_ref().map(|s| builder.create_string(s));

    // Build images vector
    let mut image_offsets = Vec::new();
    for img in &parsed.images {
        let url = builder.create_string(&img.url);
        let mime_type = img.mime_type.as_ref().map(|s| builder.create_string(s));
        let dim = img.dim.as_ref().map(|s| builder.create_string(s));
        let alt = img.alt.as_ref().map(|s| builder.create_string(s));
        let blurhash = img.blurhash.as_ref().map(|s| builder.create_string(s));
        let hash = img.hash.as_ref().map(|s| builder.create_string(s));
        let annotate_user = img.annotate_user.as_ref().map(|s| builder.create_string(s));

        // Build fallback vector
        let fallback_offsets: Vec<_> = img
            .fallback
            .iter()
            .map(|f| builder.create_string(f))
            .collect();
        let fallback = if fallback_offsets.is_empty() {
            None
        } else {
            Some(builder.create_vector(&fallback_offsets))
        };

        let args = fb::ImetaTagArgs {
            url: Some(url),
            mime_type,
            dim,
            alt,
            blurhash,
            hash,
            fallback,
            annotate_user,
        };
        image_offsets.push(fb::ImetaTag::create(builder, &args));
    }
    let images = if image_offsets.is_empty() {
        None
    } else {
        Some(builder.create_vector(&image_offsets))
    };

    // Build hashtags vector
    let hashtag_offsets: Vec<_> = parsed
        .hashtags
        .iter()
        .map(|t| builder.create_string(t))
        .collect();
    let hashtags = if hashtag_offsets.is_empty() {
        None
    } else {
        Some(builder.create_vector(&hashtag_offsets))
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

    let args = fb::Kind20ParsedArgs {
        title,
        description: Some(description),
        images,
        content_warning,
        location,
        geohash,
        hashtags,
        mentions,
    };

    let offset = fb::Kind20Parsed::create(builder, &args);
    Ok(offset)
}

/// Utility: get the first value for a tag key.
fn tag_value(tags: &[Vec<String>], key: &str) -> Option<String> {
    tags.iter()
        .find_map(|t| (t.len() >= 2 && t[0] == key).then(|| t[1].clone()))
}

/// Utility: get all values for a tag key that can repeat.
fn tag_values(tags: &[Vec<String>], key: &str) -> Vec<String> {
    tags.iter()
        .filter_map(|t| (t.len() >= 2 && t[0] == key).then(|| t[1].clone()))
        .collect()
}

/// Profile pointer for mentions.
pub struct ProfilePointer {
    pub pubkey: String,
    pub relays: Vec<String>,
}
