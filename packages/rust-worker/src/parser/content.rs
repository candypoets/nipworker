use anyhow::Result;
use nostr::nips::nip19::{self, Nip19};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::generated::nostr::fb;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaItem {
    pub image: Option<Image>,
    pub video: Option<Video>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Image {
    pub url: String,
    pub alt: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Video {
    pub url: String,
    pub thumbnail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ContentData {
    Code {
        language: Option<String>, // optional, might be extracted or None
        code: String,
    },
    Hashtag {
        tag: String,
    },
    Cashu {
        token: String, // serialized cashu token string
    },
    Image {
        url: String,
        alt: Option<String>,
    },
    Video {
        url: String,
        thumbnail: Option<String>,
    },
    MediaGroup {
        items: Vec<MediaItem>, // only Image/Video variants
    },
    Nostr {
        id: String, // e.g. note id or npub or nprofile
        relays: Vec<String>,
        author: Option<String>,
        kind: Option<u64>,
    },
    Link {
        url: String,
        title: Option<String>,
        description: Option<String>,
        image: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentBlock {
    pub block_type: String,
    pub text: String,
    pub data: Option<ContentData>,
}

impl ContentBlock {
    pub fn new(block_type: String, text: String) -> Self {
        Self {
            block_type,
            text,
            data: None,
        }
    }

    pub fn with_data(mut self, data: ContentData) -> Self {
        self.data = Some(data);
        self
    }
}

pub struct ContentParser {
    patterns: Vec<Pattern>,
}

struct Pattern {
    name: String,
    regex: Regex,
    processor: fn(&str, &regex::Captures) -> Result<ContentBlock>,
}

/// Safely truncate a string at the given byte length, ensuring we don't cut in the middle of a UTF-8 character
fn safe_truncate(s: &str, max_bytes: usize) -> &str {
    if max_bytes >= s.len() {
        return s;
    }

    // Find the largest valid character boundary at or before max_bytes
    let mut boundary = max_bytes;
    while boundary > 0 && !s.is_char_boundary(boundary) {
        boundary -= 1;
    }

    &s[..boundary]
}

impl ContentParser {
    pub fn new() -> Self {
        let patterns = vec![
            Pattern {
                name: "code".to_string(),
                regex: Regex::new(r"```([\s\S]*?)```").unwrap(),
                processor: process_code,
            },
            Pattern {
                name: "cashu".to_string(),
                regex: Regex::new(r"(cashuA[A-Za-z0-9_-]+)").unwrap(),
                processor: process_cashu,
            },
            Pattern {
                name: "hashtag".to_string(),
                regex: Regex::new(r"(^|[\s\x22\x27(])(#[a-zA-Z0-9_]+)").unwrap(),
                processor: process_hashtag,
            },
            Pattern {
                name: "image".to_string(),
                regex: Regex::new(r"(?i)(https?://\S+\.(jpg|jpeg|png|gif|webp|svg|ico)(\?\S*)?)")
                    .unwrap(),
                processor: process_image,
            },
            Pattern {
                name: "video".to_string(),
                regex: Regex::new(r"(?i)(https?://\S+\.(mp4|mov|avi|mkv|webm|m4v)(\?\S*)?)")
                    .unwrap(),
                processor: process_video,
            },
            Pattern {
                name: "nostr".to_string(),
                regex: Regex::new(r"(?i)(nostr:([a-z0-9]+)|n(event|prof|pub|addr|note)1[a-z0-9]+)")
                    .unwrap(),
                processor: process_nostr,
            },
            Pattern {
                name: "link".to_string(),
                regex: Regex::new(r"(?i)https?://[^\s\]\)]+").unwrap(),
                processor: process_link,
            },
        ];

        Self { patterns }
    }

    pub fn parse_content(&self, content: &str) -> Result<Vec<ContentBlock>> {
        let mut blocks = vec![ContentBlock::new("text".to_string(), content.to_string())];

        // Process one pattern at a time to prioritize patterns
        for pattern in &self.patterns {
            let mut new_blocks = Vec::new();

            for block in blocks {
                // Only process text blocks
                if block.block_type != "text" {
                    new_blocks.push(block);
                    continue;
                }

                // Skip empty text blocks
                if block.text.is_empty() {
                    continue;
                }

                // Find matches in this text block
                let matches: Vec<_> = pattern.regex.find_iter(&block.text).collect();
                if matches.is_empty() {
                    // No matches, keep block as is
                    new_blocks.push(block);
                    continue;
                }

                // Process matches and split the text
                let mut last_end = 0;

                for m in matches {
                    // Add text before the match if any
                    if m.start() > last_end {
                        let text_before = block.text[last_end..m.start()].to_string();
                        if !text_before.is_empty() {
                            new_blocks.push(ContentBlock::new("text".to_string(), text_before));
                        }
                    }

                    // Process and add the match
                    if let Some(caps) = pattern.regex.captures(&block.text[m.start()..m.end()]) {
                        match (pattern.processor)(m.as_str(), &caps) {
                            Ok(match_block) => new_blocks.push(match_block),
                            Err(_) => {
                                // If we can't process the match, treat it as text
                                new_blocks.push(ContentBlock::new(
                                    "text".to_string(),
                                    m.as_str().to_string(),
                                ));
                            }
                        }
                    } else {
                        // Fallback to text if regex capture fails
                        new_blocks.push(ContentBlock::new(
                            "text".to_string(),
                            m.as_str().to_string(),
                        ));
                    }

                    last_end = m.end();
                }

                // Add remaining text after last match
                if last_end < block.text.len() {
                    let remaining_text = block.text[last_end..].to_string();
                    if !remaining_text.is_empty() {
                        new_blocks.push(ContentBlock::new("text".to_string(), remaining_text));
                    }
                }
            }

            blocks = new_blocks;
        }

        // Combine adjacent text blocks
        let mut combined_blocks: Vec<ContentBlock> = Vec::new();
        for block in blocks {
            if let Some(last_block) = combined_blocks.last_mut() {
                if block.block_type == "text" && last_block.block_type == "text" {
                    // Combine with previous text block
                    last_block.text.push_str(&block.text);
                    continue;
                }
            }
            combined_blocks.push(block);
        }

        // Post-processing: group consecutive media into grids
        let processed_blocks = self.group_media(combined_blocks);

        // Remove empty text blocks
        let final_blocks: Vec<_> = processed_blocks
            .into_iter()
            .filter(|block| block.block_type != "text" || !block.text.is_empty())
            .collect();

        Ok(final_blocks)
    }

    fn group_media(&self, blocks: Vec<ContentBlock>) -> Vec<ContentBlock> {
        let mut processed_blocks = Vec::new();
        let mut media_group = Vec::new();

        for (i, block) in blocks.iter().enumerate() {
            // If this is an image or video
            if block.block_type == "image" || block.block_type == "video" {
                media_group.push(block.clone());
                continue;
            }

            // If this is whitespace or newlines between media, check what follows
            if block.block_type == "text" {
                let is_whitespace = block.text.chars().all(|c| c.is_whitespace());

                if is_whitespace && !media_group.is_empty() && i + 1 < blocks.len() {
                    let next_block = &blocks[i + 1];
                    if next_block.block_type == "image" || next_block.block_type == "video" {
                        continue;
                    }
                }
            }

            // If we have collected media and the current block breaks the sequence
            if !media_group.is_empty() {
                // Add media group if it contains more than one item
                if media_group.len() > 1 {
                    let media_texts: Vec<_> = media_group.iter().map(|m| m.text.clone()).collect();

                    processed_blocks.push(
                        ContentBlock::new("mediaGrid".to_string(), media_texts.join("\n"))
                            .with_data(ContentData::MediaGroup {
                                items: media_group
                                    .iter()
                                    .filter_map(|block| match &block.data {
                                        Some(ContentData::Image { url, alt }) => Some(MediaItem {
                                            image: Some(Image {
                                                url: url.clone(),
                                                alt: alt.clone(),
                                            }),
                                            video: None,
                                        }),
                                        Some(ContentData::Video { url, thumbnail }) => {
                                            Some(MediaItem {
                                                image: None,
                                                video: Some(Video {
                                                    url: url.clone(),
                                                    thumbnail: thumbnail.clone(),
                                                }),
                                            })
                                        }
                                        _ => None,
                                    })
                                    .collect(),
                            }),
                    );
                } else {
                    // Just add the single media item
                    processed_blocks.push(media_group[0].clone());
                }
                media_group.clear();
            }

            // Add the current non-media block
            processed_blocks.push(block.clone());
        }

        // Don't forget any remaining media
        if !media_group.is_empty() {
            if media_group.len() > 1 {
                let media_texts: Vec<_> = media_group.iter().map(|m| m.text.clone()).collect();

                processed_blocks.push(
                    ContentBlock::new("mediaGrid".to_string(), media_texts.join("\n")).with_data(
                        ContentData::MediaGroup {
                            items: media_group
                                .iter()
                                .filter_map(|block| match &block.data {
                                    Some(ContentData::Image { url, alt }) => Some(MediaItem {
                                        image: Some(Image {
                                            url: url.clone(),
                                            alt: alt.clone(),
                                        }),
                                        video: None,
                                    }),
                                    Some(ContentData::Video { url, thumbnail }) => {
                                        Some(MediaItem {
                                            image: None,
                                            video: Some(Video {
                                                url: url.clone(),
                                                thumbnail: thumbnail.clone(),
                                            }),
                                        })
                                    }
                                    _ => None,
                                })
                                .collect(),
                        },
                    ),
                );
            } else {
                processed_blocks.push(media_group[0].clone());
            }
        }

        processed_blocks
    }

    pub fn shorten_content(
        &self,
        blocks: Vec<ContentBlock>,
        max_length: usize,
        max_images: usize,
        max_lines: usize,
    ) -> Vec<ContentBlock> {
        // Be more aggressive with limits based on content size
        let adjusted_max_length = if max_length > 2000 {
            max_length / 3 // Very long content gets cut to 1/3
        } else if max_length > 1000 {
            (max_length * 2) / 3 // Medium content gets cut to 2/3
        } else {
            max_length // Short content keeps original limit
        };

        let adjusted_max_lines = if max_lines > 50 {
            max_lines / 3 // Very long gets cut to 1/3
        } else if max_lines > 20 {
            (max_lines * 2) / 3 // Medium gets cut to 2/3
        } else {
            max_lines // Short keeps original
        };

        // Check if original content has images
        let original_has_images = blocks.iter().any(|b| b.block_type == "image");

        let mut shortened_blocks = Vec::new();
        let mut current_length = 0;
        let mut current_lines = 0;
        let mut current_images = 0;
        let mut text_blocks_processed = 0;
        let mut first_image_added = false;

        // Stop processing once we hit our limits
        for block in blocks {
            if current_length >= adjusted_max_length || current_lines >= adjusted_max_lines {
                break;
            }

            match block.block_type.as_str() {
                "text" => {
                    // Limit text blocks based on content length
                    let max_text_blocks = if adjusted_max_length > 1000 { 2 } else { 4 };
                    if text_blocks_processed >= max_text_blocks {
                        break;
                    }

                    let remaining_length = adjusted_max_length.saturating_sub(current_length);
                    let remaining_lines = adjusted_max_lines.saturating_sub(current_lines);

                    // Skip if we have no meaningful remaining capacity
                    let min_remaining_length = if adjusted_max_length > 500 { 100 } else { 50 };
                    if remaining_length < min_remaining_length || remaining_lines < 1 {
                        break;
                    }

                    let mut text = block.text.clone();
                    let mut needs_truncation = false;

                    // First, aggressively truncate by lines
                    let lines: Vec<&str> = text.lines().collect();
                    if lines.len() > remaining_lines {
                        text = lines
                            .into_iter()
                            .take(remaining_lines)
                            .collect::<Vec<_>>()
                            .join("\n");
                        needs_truncation = true;
                    }

                    // Then truncate by character length if still too long
                    if text.len() > remaining_length {
                        // Reserve space for "..." suffix
                        let target_length = remaining_length.saturating_sub(3);
                        text = safe_truncate(&text, target_length).to_string();
                        needs_truncation = true;
                    }

                    // Truncate individual lines that are too long
                    let max_line_length = if adjusted_max_length > 1000 { 150 } else { 200 };
                    let truncated_lines: Vec<String> = text
                        .lines()
                        .take(remaining_lines) // Ensure we don't exceed line limit
                        .map(|line| {
                            if line.len() > max_line_length {
                                format!(
                                    "{}...",
                                    safe_truncate(line, max_line_length.saturating_sub(3))
                                )
                            } else {
                                line.to_string()
                            }
                        })
                        .collect();

                    if truncated_lines.len() < text.lines().count()
                        || truncated_lines.iter().any(|line| line.ends_with("..."))
                    {
                        needs_truncation = true;
                    }

                    text = truncated_lines.join("\n");

                    // Add ellipsis if we truncated anything
                    if needs_truncation && !text.ends_with("...") {
                        text = format!("{}...", text);
                    }

                    let text_len = text.len();
                    let text_lines = text.lines().count();

                    shortened_blocks.push(ContentBlock {
                        block_type: block.block_type,
                        text,
                        data: block.data,
                    });

                    current_length += text_len;
                    current_lines += text_lines;
                    text_blocks_processed += 1;
                }
                "image" => {
                    // Always include the first image if original content has images
                    if !first_image_added && original_has_images {
                        shortened_blocks.push(block);
                        current_images += 1;
                        first_image_added = true;
                    } else if current_images < max_images {
                        shortened_blocks.push(block);
                        current_images += 1;
                    }
                }
                _ => {
                    // For other block types (links, hashtags, etc.), add them but count their length
                    if current_length + block.text.len() <= adjusted_max_length {
                        current_length += block.text.len();
                        shortened_blocks.push(block);
                    }
                }
            }
        }

        shortened_blocks
    }
}

impl Default for ContentParser {
    fn default() -> Self {
        Self::new()
    }
}

// Pattern processors

fn process_code(text: &str, caps: &regex::Captures) -> Result<ContentBlock> {
    let code = caps.get(1).map_or("", |m| m.as_str());
    Ok(
        ContentBlock::new("code".to_string(), text.to_string()).with_data(ContentData::Code {
            language: None,
            code: code.to_string(),
        }),
    )
}

fn process_cashu(text: &str, _caps: &regex::Captures) -> Result<ContentBlock> {
    Ok(
        ContentBlock::new("cashu".to_string(), text.to_string()).with_data(ContentData::Cashu {
            token: text.to_string(),
        }),
    )
}

fn process_hashtag(text: &str, caps: &regex::Captures) -> Result<ContentBlock> {
    // Now we have capture groups: full match, prefix, hashtag
    let prefix = caps.get(1).map_or("", |m| m.as_str());
    let hashtag = caps.get(2).map_or("", |m| m.as_str());
    let tag = if hashtag.starts_with('#') {
        &hashtag[1..]
    } else {
        hashtag
    };

    // Include the prefix in the text but only process the hashtag part
    let full_text = format!("{}{}", prefix, hashtag);
    Ok(
        ContentBlock::new("hashtag".to_string(), full_text).with_data(ContentData::Hashtag {
            tag: tag.to_string(),
        }),
    )
}

fn process_image(text: &str, _caps: &regex::Captures) -> Result<ContentBlock> {
    Ok(
        ContentBlock::new("image".to_string(), text.to_string()).with_data(ContentData::Image {
            url: text.to_string(),
            alt: None,
        }),
    )
}

fn process_video(text: &str, _caps: &regex::Captures) -> Result<ContentBlock> {
    Ok(
        ContentBlock::new("video".to_string(), text.to_string()).with_data(ContentData::Video {
            url: text.to_string(),
            thumbnail: None,
        }),
    )
}

fn process_nostr(text: &str, _caps: &regex::Captures) -> Result<ContentBlock> {
    let entity = if text.to_lowercase().starts_with("nostr:") {
        // Extract the identifier after nostr:
        &text[6..]
    } else {
        text
    };

    // Try to decode the identifier
    match nip19::FromBech32::from_bech32(entity) {
        Ok(decoded) => {
            let (prefix, relays, author, kind) = match decoded {
                Nip19::Pubkey(pk) => ("npub", Vec::<String>::new(), Some(pk.to_string()), None),
                Nip19::Secret(sk) => ("nsec", Vec::<String>::new(), Some(sk.to_string()), None),
                // Nip19::EncryptedSecret(enc_sk) => (
                //     "ncryptsec",
                //     Vec::new(),
                //     None,
                //     None,
                // ),
                Nip19::EventId(note) => ("note", Vec::<String>::new(), None, None),
                Nip19::Profile(profile) => (
                    "nprofile",
                    profile.relays.into_iter().map(|r| r.to_string()).collect(),
                    Some(profile.public_key.to_string()),
                    None,
                ),
                Nip19::Event(event) => (
                    "nevent",
                    event.relays.into_iter().map(|r| r.to_string()).collect(),
                    event.author.map(|pk| pk.to_string()),
                    None,
                ),
                Nip19::Coordinate(coord) => (
                    "naddr",
                    coord.relays.into_iter().map(|r| r.to_string()).collect(),
                    Some(coord.public_key.to_string()),
                    Some(coord.kind.as_u64()),
                ),
            };

            Ok(
                ContentBlock::new(prefix.to_string(), text.to_string()).with_data(
                    ContentData::Nostr {
                        id: entity.to_string(),
                        relays,
                        author,
                        kind,
                    },
                ),
            )
        }
        Err(_) => {
            // If we can't decode, treat as text
            Ok(ContentBlock::new("text".to_string(), text.to_string()))
        }
    }
}

fn process_link(text: &str, _caps: &regex::Captures) -> Result<ContentBlock> {
    let url = if text.to_lowercase().starts_with("http") {
        text.to_string()
    } else {
        format!("https://{}", text)
    };

    // Create a placeholder preview
    let preview = get_link_preview(&url);

    Ok(
        ContentBlock::new("link".to_string(), text.to_string()).with_data(ContentData::Link {
            url: text.to_string(),
            title: Some(
                preview["title"]
                    .as_str()
                    .unwrap_or("Link Preview")
                    .to_string(),
            ),
            description: Some(
                preview["description"]
                    .as_str()
                    .unwrap_or("Link preview not implemented")
                    .to_string(),
            ),
            image: preview["url"].as_str().map(|s| s.to_string()),
        }),
    )
}

fn get_link_preview(url: &str) -> serde_json::Value {
    // Placeholder for link preview functionality
    serde_json::json!({
        "url": format!("https://proxy.nuts.cash/?url={}", url),
        "title": "Link Preview",
        "description": "Link preview not implemented"
    })
}

// Public function to parse content
pub fn parse_content(content: &str) -> Result<Vec<ContentBlock>> {
    let parser = ContentParser::new();
    parser.parse_content(content)
}

pub fn serialize_content_data<'a, A: flatbuffers::Allocator + 'a>(
    builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
    data: &ContentData,
) -> (
    fb::ContentData,
    Option<flatbuffers::WIPOffset<flatbuffers::UnionWIPOffset>>,
) {
    match data {
        ContentData::Code { language, code } => {
            let code_off = builder.create_string(code);
            let lang_off = language.as_ref().map(|l| builder.create_string(l));
            let code_fb = fb::CodeData::create(
                builder,
                &fb::CodeDataArgs {
                    language: lang_off,
                    code: Some(code_off),
                },
            );
            (fb::ContentData::CodeData, Some(code_fb.as_union_value()))
        }
        ContentData::Hashtag { tag } => {
            let tag_off = builder.create_string(tag);
            let hashtag_fb =
                fb::HashtagData::create(builder, &fb::HashtagDataArgs { tag: Some(tag_off) });
            (
                fb::ContentData::HashtagData,
                Some(hashtag_fb.as_union_value()),
            )
        }
        ContentData::Cashu { token } => {
            let token_off = builder.create_string(token);
            let cashu_fb = fb::CashuData::create(
                builder,
                &fb::CashuDataArgs {
                    token: Some(token_off),
                },
            );
            (fb::ContentData::CashuData, Some(cashu_fb.as_union_value()))
        }
        ContentData::Image { url, alt } => {
            let url_off = builder.create_string(url);
            let alt_off = alt.as_ref().map(|a| builder.create_string(a));
            let image_fb = fb::ImageData::create(
                builder,
                &fb::ImageDataArgs {
                    url: Some(url_off),
                    alt: alt_off,
                },
            );
            (fb::ContentData::ImageData, Some(image_fb.as_union_value()))
        }
        ContentData::Video { url, thumbnail } => {
            let url_off = builder.create_string(url);
            let thumb_off = thumbnail.as_ref().map(|t| builder.create_string(t));
            let video_fb = fb::VideoData::create(
                builder,
                &fb::VideoDataArgs {
                    url: Some(url_off),
                    thumbnail: thumb_off,
                },
            );
            (fb::ContentData::VideoData, Some(video_fb.as_union_value()))
        }
        ContentData::MediaGroup { items } => {
            let fb_items: Vec<_> = items
                .iter()
                .map(|item| {
                    // Serialize inner ImageData if present
                    let img_off = item.image.as_ref().map(|img| {
                        let url_off = builder.create_string(&img.url);
                        let alt_off = img.alt.as_ref().map(|a| builder.create_string(a));
                        fb::ImageData::create(
                            builder,
                            &fb::ImageDataArgs {
                                url: Some(url_off),
                                alt: alt_off,
                            },
                        )
                    });

                    // Serialize inner VideoData if present
                    let vid_off = item.video.as_ref().map(|vid| {
                        let url_off = builder.create_string(&vid.url);
                        let thumb_off = vid.thumbnail.as_ref().map(|t| builder.create_string(t));
                        fb::VideoData::create(
                            builder,
                            &fb::VideoDataArgs {
                                url: Some(url_off),
                                thumbnail: thumb_off,
                            },
                        )
                    });

                    // Build MediaItem table with one side set
                    fb::MediaItem::create(
                        builder,
                        &fb::MediaItemArgs {
                            image: img_off,
                            video: vid_off,
                        },
                    )
                })
                .collect();

            let items_vec = builder.create_vector(&fb_items);

            let mg_fb = fb::MediaGroupData::create(
                builder,
                &fb::MediaGroupDataArgs {
                    items: Some(items_vec),
                },
            );

            (
                fb::ContentData::MediaGroupData,
                Some(mg_fb.as_union_value()),
            )
        }
        ContentData::Nostr {
            id,
            relays,
            author,
            kind,
        } => {
            let id_off = builder.create_string(id);
            let relays_strs: Vec<_> = relays.iter().map(|r| builder.create_string(r)).collect();
            let relays_off = Some(builder.create_vector(&relays_strs));
            let author_off = author.as_ref().map(|a| builder.create_string(a));
            let nostr_fb = fb::NostrData::create(
                builder,
                &fb::NostrDataArgs {
                    id: Some(id_off),
                    relays: relays_off,
                    author: author_off,
                    kind: kind.unwrap_or(0),
                },
            );
            (fb::ContentData::NostrData, Some(nostr_fb.as_union_value()))
        }
        ContentData::Link {
            url,
            title,
            description,
            image,
        } => {
            let url_off = builder.create_string(url);
            let title_off = title.as_ref().map(|t| builder.create_string(t));
            let desc_off = description.as_ref().map(|d| builder.create_string(d));
            let img_off = image.as_ref().map(|i| builder.create_string(i));
            let link_fb = fb::LinkPreviewData::create(
                builder,
                &fb::LinkPreviewDataArgs {
                    url: Some(url_off),
                    title: title_off,
                    description: desc_off,
                    image: img_off,
                },
            );
            (
                fb::ContentData::LinkPreviewData,
                Some(link_fb.as_union_value()),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_plain_text() {
        let content = "This is just plain text";
        let result = parse_content(content).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].block_type, "text");
        assert_eq!(result[0].text, content);
    }

    #[test]
    fn test_parse_code_block() {
        let content = "Here is some code: ```var x = 10;``` and more text";
        let result = parse_content(content).unwrap();
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].block_type, "text");
        assert_eq!(result[1].block_type, "code");
        assert_eq!(result[2].block_type, "text");
    }

    #[test]
    fn test_parse_hashtag() {
        let content = "I love #bitcoin and #lightning";
        let result = parse_content(content).unwrap();
        assert_eq!(result.len(), 4);
        assert_eq!(result[1].block_type, "hashtag");
        assert_eq!(result[3].block_type, "hashtag");
    }

    #[test]
    fn test_parse_image() {
        let content = "Check this image: https://example.com/image.jpg";
        let result = parse_content(content).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[1].block_type, "image");
    }

    #[test]
    fn test_parse_mixed_content() {
        let content = "Hello #world!\nCheck out https://example.com";
        let result = parse_content(content).unwrap();
        assert!(result.len() >= 3);

        let has_hashtag = result.iter().any(|b| b.block_type == "hashtag");
        let has_link = result.iter().any(|b| b.block_type == "link");
        assert!(has_hashtag);
        assert!(has_link);
    }
}
