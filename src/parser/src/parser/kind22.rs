use crate::parser::{Parser, ParserError, Result};
use shared::{
    generated::nostr::*,
    types::{network::Request, Event},
};

/// Video variant (different resolutions/qualities)
pub struct VideoVariant {
    pub url: String,
    pub mime_type: Option<String>,
    pub dim: Option<String>,
    pub blurhash: Option<String>,
    pub hash: Option<String>,
    pub duration: Option<f32>,
    pub bitrate: Option<u64>,
    pub image: Option<String>,
    pub fallback: Vec<String>,
}

/// Parsed representation for NIP-71 (kind 22) short-form video events.
pub struct Kind22Parsed {
    /// Title from "title" tag (required)
    pub title: String,
    /// Description/content text
    pub description: String,
    /// Video variants from "imeta" tags
    pub videos: Vec<VideoVariant>,
    /// Alt text for accessibility
    pub alt: Option<String>,
    /// Content warning from "content-warning" tag
    pub content_warning: Option<String>,
    /// Duration in seconds
    pub duration: Option<f32>,
    /// Published timestamp from "published_at" tag
    pub published_at: Option<u64>,
    /// Hashtags from "t" tags
    pub hashtags: Vec<String>,
    /// Participants from "p" tags
    pub participants: Vec<ProfilePointer>,
}

/// Profile pointer for participants.
pub struct ProfilePointer {
    pub pubkey: String,
    pub relays: Vec<String>,
}

impl Parser {
    /// Parse a kind 22 short-form video event.
    pub fn parse_kind_22(&self, event: &Event) -> Result<(Kind22Parsed, Option<Vec<Request>>)> {
        if event.kind != 22 {
            return Err(ParserError::Other("event is not kind 22".to_string()));
        }

        // Extract metadata from tags
        let title = tag_value(&event.tags, "title").unwrap_or_default();
        let alt = tag_value(&event.tags, "alt");
        let content_warning = tag_value(&event.tags, "content-warning");
        let duration = tag_value(&event.tags, "duration")
            .and_then(|s| s.parse::<f32>().ok());
        let published_at = tag_value(&event.tags, "published_at")
            .and_then(|s| s.parse::<u64>().ok());
        let hashtags = tag_values(&event.tags, "t");

        // Extract imeta tags for videos
        let videos = extract_video_imeta_tags(&event.tags);

        // Extract p-tag participants
        let participants = extract_participants(&event.tags);

        let parsed = Kind22Parsed {
            title,
            description: event.content.clone(),
            videos,
            alt,
            content_warning,
            duration,
            published_at,
            hashtags,
            participants,
        };

        Ok((parsed, None))
    }
}

/// Extract imeta tags from event tags for videos.
fn extract_video_imeta_tags(tags: &[Vec<String>]) -> Vec<VideoVariant> {
    let mut videos = Vec::new();

    for tag in tags {
        if tag.len() < 2 || tag[0] != "imeta" {
            continue;
        }

        let mut url = None;
        let mut mime_type = None;
        let mut dim = None;
        let mut blurhash = None;
        let mut hash = None;
        let mut duration = None;
        let mut bitrate = None;
        let mut image = None;
        let mut fallback = Vec::new();

        // Parse each field in the imeta tag (skip tag[0] which is "imeta")
        for field in &tag[1..] {
            if let Some((key, value)) = field.split_once(' ') {
                let value = value.trim();
                match key {
                    "url" => url = Some(value.to_string()),
                    "m" => mime_type = Some(value.to_string()),
                    "dim" => dim = Some(value.to_string()),
                    "blurhash" => blurhash = Some(value.to_string()),
                    "x" => hash = Some(value.to_string()),
                    "duration" => duration = value.parse::<f32>().ok(),
                    "bitrate" => bitrate = value.parse::<u64>().ok(),
                    "image" => image = Some(value.to_string()),
                    "fallback" => fallback.push(value.to_string()),
                    _ => {} // Ignore unknown fields
                }
            }
        }

        // Only add if we have a URL
        if let Some(url) = url {
            videos.push(VideoVariant {
                url,
                mime_type,
                dim,
                blurhash,
                hash,
                duration,
                bitrate,
                image,
                fallback,
            });
        }
    }

    videos
}

/// Extract p-tag participants.
fn extract_participants(tags: &[Vec<String>]) -> Vec<ProfilePointer> {
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

/// Build the FlatBuffer for `Kind22Parsed`.
pub fn build_flatbuffer<'a, A: flatbuffers::Allocator + 'a>(
    parsed: &Kind22Parsed,
    builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
) -> Result<flatbuffers::WIPOffset<fb::Kind22Parsed<'a>>> {
    // Required strings
    let title = builder.create_string(&parsed.title);
    let description = builder.create_string(&parsed.description);

    // Optional strings
    let alt = parsed.alt.as_ref().map(|s| builder.create_string(s));
    let content_warning = parsed
        .content_warning
        .as_ref()
        .map(|s| builder.create_string(s));

    // Build videos vector
    let mut video_offsets = Vec::new();
    for vid in &parsed.videos {
        let url = builder.create_string(&vid.url);
        let mime_type = vid.mime_type.as_ref().map(|s| builder.create_string(s));
        let dim = vid.dim.as_ref().map(|s| builder.create_string(s));
        let blurhash = vid.blurhash.as_ref().map(|s| builder.create_string(s));
        let hash = vid.hash.as_ref().map(|s| builder.create_string(s));
        let image = vid.image.as_ref().map(|s| builder.create_string(s));

        // Build fallback vector
        let fallback_offsets: Vec<_> = vid
            .fallback
            .iter()
            .map(|f| builder.create_string(f))
            .collect();
        let fallback = if fallback_offsets.is_empty() {
            None
        } else {
            Some(builder.create_vector(&fallback_offsets))
        };

        let args = fb::VideoVariantArgs {
            url: Some(url),
            mime_type,
            dim,
            blurhash,
            hash,
            duration: vid.duration.unwrap_or(0.0),
            bitrate: vid.bitrate.unwrap_or(0),
            image,
            fallback,
        };
        video_offsets.push(fb::VideoVariant::create(builder, &args));
    }
    let videos = if video_offsets.is_empty() {
        None
    } else {
        Some(builder.create_vector(&video_offsets))
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

    // Build participants vector
    let mut participant_offsets = Vec::new();
    for p in &parsed.participants {
        let pubkey = builder.create_string(&p.pubkey);
        let relay_offsets: Vec<_> = p.relays.iter().map(|r| builder.create_string(r)).collect();
        let relays = if relay_offsets.is_empty() {
            None
        } else {
            Some(builder.create_vector(&relay_offsets))
        };
        let args = fb::ProfilePointerArgs {
            public_key: Some(pubkey),
            relays,
        };
        participant_offsets.push(fb::ProfilePointer::create(builder, &args));
    }
    let participants = if participant_offsets.is_empty() {
        None
    } else {
        Some(builder.create_vector(&participant_offsets))
    };

    let args = fb::Kind22ParsedArgs {
        title: Some(title),
        description: Some(description),
        videos,
        alt,
        content_warning,
        duration: parsed.duration.unwrap_or(0.0),
        published_at: parsed.published_at.unwrap_or(0),
        hashtags,
        participants,
    };

    let offset = fb::Kind22Parsed::create(builder, &args);
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
