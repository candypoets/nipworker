use crate::parser::Result;
use crate::types::nostr::nips::nip19::{self, Nip19};

use crate::generated::nostr::fb;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaItem {
    pub image: Option<Image>,
    pub video: Option<Video>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub url: String,
    pub alt: Option<String>,
    pub dim: Option<String>, // Dimensions in "widthxheight" format (e.g., "1920x1080")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Video {
    pub url: String,
    pub thumbnail: Option<String>,
    pub dim: Option<String>, // Dimensions in "widthxheight" format (e.g., "1920x1080")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentData {
    Code {
        language: Option<String>,
        code: String,
    },
    Hashtag {
        tag: String,
    },
    Link {
        url: String,
    },
    Image {
        url: String,
        alt: Option<String>,
        dim: Option<String>,
    },
    Video {
        url: String,
        thumbnail: Option<String>,
        dim: Option<String>,
    },
    Nostr {
        entity: String,
        data: Option<String>,
    },
    Cashu {
        token: String,
    },
    Emoji {
        shortcode: String,
        url: Option<String>,
    },
    Mention {
        pubkey: String,
        relays: Vec<String>,
    },
    Relay {
        url: String,
    },
    MediaGrid {
        items: Vec<MediaItem>,
    },
    Quote {
        event_id: String,
        relays: Vec<String>,
        author: Option<String>,
    },
    LinkPreview {
        url: String,
        title: Option<String>,
        description: Option<String>,
        image: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
    emoji_map: std::collections::HashMap<String, (String, Option<String>)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImetaData {
    pub url: String,
    pub alt: Option<String>,
    pub dim: Option<String>, // Dimensions in "widthxheight" format (e.g., "1920x1080")
    pub blurhash: Option<String>,
    pub mime_type: Option<String>,
}

/// Safely truncate a string at the given byte length, ensuring we don't cut in the middle of a UTF-8 character
fn safe_truncate(s: &str, max_bytes: usize) -> &str {
    if max_bytes >= s.len() {
        return s;
    }

    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

impl ContentParser {
    pub fn new() -> Self {
        Self {
            emoji_map: std::collections::HashMap::new(),
        }
    }

    pub fn with_emojis(emoji_tags: &[Vec<String>]) -> Self {
        let mut emoji_map = std::collections::HashMap::new();
        for tag in emoji_tags {
            if tag.len() >= 2 {
                let shortcode = tag[1].clone();
                let url = tag.get(2).cloned();
                emoji_map.insert(shortcode, (String::new(), url));
            }
        }
        Self { emoji_map }
    }

    pub fn parse_content(&self, content: &str) -> Result<Vec<ContentBlock>> {
        Ok(vec![ContentBlock::new("text".to_string(), content.to_string())])
    }

    pub fn shorten_content(
        &self,
        blocks: Vec<ContentBlock>,
        _max_length: usize,
        _max_images: usize,
        _max_lines: usize,
    ) -> Vec<ContentBlock> {
        blocks
    }
}

impl Default for ContentParser {
    fn default() -> Self {
        Self::new()
    }
}

pub struct LinkPreview {
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub image: Option<String>,
}

impl LinkPreview {
    pub fn new(url: &str) -> Self {
        Self {
            url: url.to_string(),
            title: None,
            description: None,
            image: None,
        }
    }
}

fn get_link_preview(url: &str) -> LinkPreview {
    LinkPreview::new(url)
}

// Public function to parse content with emoji support
pub fn parse_content(content: &str, emoji_tags: &[Vec<String>]) -> Result<Vec<ContentBlock>> {
    let parser = ContentParser::with_emojis(emoji_tags);
    parser.parse_content(content)
}

/// Enrich media (image/video) blocks with imeta data after parsing
pub fn enrich_media_with_imeta(
    blocks: &mut [ContentBlock],
    imeta_map: &std::collections::HashMap<String, ImetaData>,
) {
    for block in blocks.iter_mut() {
        if block.block_type == "image" {
            if let Some(ContentData::Image { url, alt, dim }) = &mut block.data {
                if let Some(imeta) = imeta_map.get(url) {
                    if alt.is_none() && imeta.alt.is_some() {
                        *alt = imeta.alt.clone();
                    }
                    if dim.is_none() && imeta.dim.is_some() {
                        *dim = imeta.dim.clone();
                    }
                }
            }
        } else if block.block_type == "video" {
            if let Some(ContentData::Video { url, thumbnail, dim }) = &mut block.data {
                if let Some(imeta) = imeta_map.get(url) {
                    if thumbnail.is_none() && imeta.alt.is_some() {
                        *thumbnail = imeta.alt.clone();
                    }
                    if dim.is_none() && imeta.dim.is_some() {
                        *dim = imeta.dim.clone();
                    }
                }
            }
        }
    }
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
        ContentData::Link { url } => {
            let url_off = builder.create_string(url);
            let link_fb = fb::LinkPreviewData::create(
                builder,
                &fb::LinkPreviewDataArgs {
                    url: Some(url_off),
                    title: None,
                    description: None,
                    image: None,
                },
            );
            (fb::ContentData::LinkPreviewData, Some(link_fb.as_union_value()))
        }
        ContentData::Image { url, alt, dim } => {
            let url_off = builder.create_string(url);
            let alt_off = alt.as_ref().map(|a| builder.create_string(a));
            let dim_off = dim.as_ref().map(|d| builder.create_string(d));
            let image_fb = fb::ImageData::create(
                builder,
                &fb::ImageDataArgs {
                    url: Some(url_off),
                    alt: alt_off,
                    dim: dim_off,
                },
            );
            (fb::ContentData::ImageData, Some(image_fb.as_union_value()))
        }
        ContentData::Video { url, thumbnail, dim } => {
            let url_off = builder.create_string(url);
            let thumb_off = thumbnail.as_ref().map(|t| builder.create_string(t));
            let dim_off = dim.as_ref().map(|d| builder.create_string(d));
            let video_fb = fb::VideoData::create(
                builder,
                &fb::VideoDataArgs {
                    url: Some(url_off),
                    thumbnail: thumb_off,
                    dim: dim_off,
                },
            );
            (fb::ContentData::VideoData, Some(video_fb.as_union_value()))
        }
        ContentData::Nostr { entity, data } => {
            let entity_off = builder.create_string(entity);
            let data_off = data.as_ref().map(|d| builder.create_string(d));
            let nostr_fb = fb::NostrData::create(
                builder,
                &fb::NostrDataArgs {
                    id: data_off,
                    entity: Some(entity_off),
                    relays: None,
                    author: None,
                    kind: 0,
                },
            );
            (fb::ContentData::NostrData, Some(nostr_fb.as_union_value()))
        }
        ContentData::Cashu { token } => {
            let token_off = builder.create_string(token);
            let cashu_fb =
                fb::CashuData::create(builder, &fb::CashuDataArgs { token: Some(token_off) });
            (fb::ContentData::CashuData, Some(cashu_fb.as_union_value()))
        }
        ContentData::Emoji { shortcode, url } => {
            let shortcode_off = builder.create_string(shortcode);
            let url_off = url.as_ref().map(|u| builder.create_string(u));
            let emoji_fb = fb::EmojiData::create(
                builder,
                &fb::EmojiDataArgs {
                    shortcode: Some(shortcode_off),
                    url: url_off,
                    emoji_set: None,
                },
            );
            (fb::ContentData::EmojiData, Some(emoji_fb.as_union_value()))
        }
        ContentData::Mention { .. } => (fb::ContentData::NONE, None),
        ContentData::Relay { .. } => (fb::ContentData::NONE, None),
        ContentData::MediaGrid { items } => {
            let mut item_offsets = Vec::new();
            for item in items {
                let image_off = item.image.as_ref().map(|img| {
                    let url_off = builder.create_string(&img.url);
                    let alt_off = img.alt.as_ref().map(|a| builder.create_string(a));
                    let dim_off = img.dim.as_ref().map(|d| builder.create_string(d));
                    fb::ImageData::create(
                        builder,
                        &fb::ImageDataArgs {
                            url: Some(url_off),
                            alt: alt_off,
                            dim: dim_off,
                        },
                    )
                });
                let video_off = item.video.as_ref().map(|vid| {
                    let url_off = builder.create_string(&vid.url);
                    let thumb_off = vid.thumbnail.as_ref().map(|t| builder.create_string(t));
                    let dim_off = vid.dim.as_ref().map(|d| builder.create_string(d));
                    fb::VideoData::create(
                        builder,
                        &fb::VideoDataArgs {
                            url: Some(url_off),
                            thumbnail: thumb_off,
                            dim: dim_off,
                        },
                    )
                });
                let media_item_fb = fb::MediaItem::create(
                    builder,
                    &fb::MediaItemArgs {
                        image: image_off,
                        video: video_off,
                    },
                );
                item_offsets.push(media_item_fb);
            }
            let items_vec = builder.create_vector(&item_offsets);
            let grid_fb =
                fb::MediaGroupData::create(builder, &fb::MediaGroupDataArgs { items: Some(items_vec) });
            (fb::ContentData::MediaGroupData, Some(grid_fb.as_union_value()))
        }
        ContentData::Quote { .. } => (fb::ContentData::NONE, None),
        ContentData::LinkPreview { url, title, description, image } => {
            let url_off = builder.create_string(url);
            let title_off = title.as_ref().map(|t| builder.create_string(t));
            let desc_off = description.as_ref().map(|d| builder.create_string(d));
            let image_off = image.as_ref().map(|i| builder.create_string(i));
            let preview_fb = fb::LinkPreviewData::create(
                builder,
                &fb::LinkPreviewDataArgs {
                    url: Some(url_off),
                    title: title_off,
                    description: desc_off,
                    image: image_off,
                },
            );
            (
                fb::ContentData::LinkPreviewData,
                Some(preview_fb.as_union_value()),
            )
        }
    }
}

pub fn serialize_content_block<'a, A: flatbuffers::Allocator + 'a>(
    builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
    block: &ContentBlock,
) -> flatbuffers::WIPOffset<fb::ContentBlock<'a>> {
    let type_off = builder.create_string(&block.block_type);
    let text_off = builder.create_string(&block.text);

    let (data_type, data_off) = if let Some(ref data) = block.data {
        serialize_content_data(builder, data)
    } else {
        (fb::ContentData::NONE, None)
    };

    fb::ContentBlock::create(
        builder,
        &fb::ContentBlockArgs {
            type_: Some(type_off),
            text: Some(text_off),
            data_type,
            data: data_off,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_content_basic() {
        let result = parse_content("Hello world", &[]).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].block_type, "text");
        assert_eq!(result[0].text, "Hello world");
    }
}
