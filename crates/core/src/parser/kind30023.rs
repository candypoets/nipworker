use crate::parser::{Parser, ParserError, Result};
use crate::{
    generated::nostr::*,
    types::{network::Request, Event},
}; // brings `fb::...` into scope
use pulldown_cmark::{CodeBlockKind, Event as MarkdownEvent, HeadingLevel, Options, Parser as MarkdownParser, Tag, TagEnd};
use regex::Regex;

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
    /// Markdown-first parsed document blocks
    pub article_blocks: Vec<ArticleBlock>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArticleBlock {
    pub block_type: fb::ArticleBlockType,
    pub text: Option<String>,
    pub url: Option<String>,
    pub language: Option<String>,
    pub depth: u8,
    pub ordered: bool,
    pub start: u64,
    pub inlines: Vec<ArticleInline>,
    pub children: Vec<ArticleBlock>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArticleInline {
    pub inline_type: fb::ArticleInlineType,
    pub text: Option<String>,
    pub url: Option<String>,
    pub tag: Option<String>,
    pub entity: Option<ArticleEntity>,
    pub children: Vec<ArticleInline>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArticleEntity {
    pub entity: String,
    pub id: Option<String>,
    pub relays: Vec<String>,
    pub author: Option<String>,
    pub kind: Option<u64>,
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
        let article_blocks = parse_article_markdown(&event.content);

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
            article_blocks,
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
    let article_block_offsets: Vec<_> = parsed
        .article_blocks
        .iter()
        .map(|block| build_article_block(block, builder))
        .collect();
    let article_blocks = if article_block_offsets.is_empty() {
        None
    } else {
        Some(builder.create_vector(&article_block_offsets))
    };

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
        article_blocks,
    };

    let offset = fb::Kind30023Parsed::create(builder, &args);
    Ok(offset)
}

fn parse_article_markdown(content: &str) -> Vec<ArticleBlock> {
    let parser = MarkdownParser::new_ext(
        content,
        Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_TABLES
            | Options::ENABLE_FOOTNOTES
            | Options::ENABLE_TASKLISTS,
    );
    let mut block_stack: Vec<ArticleBlock> = Vec::new();
    let mut inline_stack: Vec<ArticleInline> = Vec::new();
    let mut blocks: Vec<ArticleBlock> = Vec::new();

    for event in parser {
        match event {
            MarkdownEvent::Start(tag) => match tag {
                Tag::Paragraph => block_stack.push(article_block(fb::ArticleBlockType::Paragraph)),
                Tag::Heading { level, .. } => {
                    let mut block = article_block(fb::ArticleBlockType::Heading);
                    block.depth = heading_depth(level);
                    block_stack.push(block);
                }
                Tag::BlockQuote(_) => block_stack.push(article_block(fb::ArticleBlockType::Blockquote)),
                Tag::List(start) => {
                    let mut block = article_block(fb::ArticleBlockType::List);
                    block.ordered = start.is_some();
                    block.start = start.unwrap_or(0);
                    block_stack.push(block);
                }
                Tag::Item => block_stack.push(article_block(fb::ArticleBlockType::ListItem)),
                Tag::CodeBlock(kind) => {
                    let mut block = article_block(fb::ArticleBlockType::CodeBlock);
                    if let CodeBlockKind::Fenced(language) = kind {
                        if !language.is_empty() {
                            block.language = Some(language.to_string());
                        }
                    }
                    block_stack.push(block);
                }
                Tag::Emphasis => inline_stack.push(article_inline(fb::ArticleInlineType::Emphasis)),
                Tag::Strong => inline_stack.push(article_inline(fb::ArticleInlineType::Strong)),
                Tag::Link { dest_url, .. } => {
                    let mut inline = article_inline(fb::ArticleInlineType::Link);
                    inline.url = Some(dest_url.to_string());
                    inline_stack.push(inline);
                }
                Tag::Image { dest_url, title, .. } => {
                    let mut inline = article_inline(fb::ArticleInlineType::Image);
                    inline.url = Some(dest_url.to_string());
                    if !title.is_empty() {
                        inline.text = Some(title.to_string());
                    }
                    inline_stack.push(inline);
                }
                _ => {}
            },
            MarkdownEvent::End(tag) => match tag {
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::BlockQuote(_)
                | TagEnd::List(_)
                | TagEnd::Item
                | TagEnd::CodeBlock => {
                    if let Some(block) = block_stack.pop() {
                        append_block(&mut block_stack, &mut blocks, block);
                    }
                }
                TagEnd::Emphasis | TagEnd::Strong | TagEnd::Link | TagEnd::Image => {
                    if let Some(inline) = inline_stack.pop() {
                        append_inline(&mut block_stack, &mut inline_stack, inline);
                    }
                }
                _ => {}
            },
            MarkdownEvent::Text(text) => {
                let value = text.to_string();
                if current_block_type(&block_stack) == Some(fb::ArticleBlockType::CodeBlock) {
                    append_text_to_current_block(&mut block_stack, &value);
                } else {
                    for inline in parse_text_inlines(&value) {
                        append_inline(&mut block_stack, &mut inline_stack, inline);
                    }
                }
            }
            MarkdownEvent::Code(code) => {
                let mut inline = article_inline(fb::ArticleInlineType::Code);
                inline.text = Some(code.to_string());
                append_inline(&mut block_stack, &mut inline_stack, inline);
            }
            MarkdownEvent::SoftBreak => {
                append_inline(
                    &mut block_stack,
                    &mut inline_stack,
                    article_inline(fb::ArticleInlineType::SoftBreak),
                );
            }
            MarkdownEvent::HardBreak => {
                append_inline(
                    &mut block_stack,
                    &mut inline_stack,
                    article_inline(fb::ArticleInlineType::LineBreak),
                );
            }
            MarkdownEvent::Rule => {
                append_block(
                    &mut block_stack,
                    &mut blocks,
                    article_block(fb::ArticleBlockType::ThematicBreak),
                );
            }
            MarkdownEvent::Html(html) | MarkdownEvent::InlineHtml(html) => {
                let mut inline = article_inline(fb::ArticleInlineType::Html);
                inline.text = Some(html.to_string());
                append_inline(&mut block_stack, &mut inline_stack, inline);
            }
            _ => {}
        }
    }

    while let Some(inline) = inline_stack.pop() {
        append_inline(&mut block_stack, &mut Vec::new(), inline);
    }
    while let Some(block) = block_stack.pop() {
        append_block(&mut block_stack, &mut blocks, block);
    }

    blocks
}

fn article_block(block_type: fb::ArticleBlockType) -> ArticleBlock {
    ArticleBlock {
        block_type,
        text: None,
        url: None,
        language: None,
        depth: 0,
        ordered: false,
        start: 0,
        inlines: Vec::new(),
        children: Vec::new(),
    }
}

fn article_inline(inline_type: fb::ArticleInlineType) -> ArticleInline {
    ArticleInline {
        inline_type,
        text: None,
        url: None,
        tag: None,
        entity: None,
        children: Vec::new(),
    }
}

fn heading_depth(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn current_block_type(block_stack: &[ArticleBlock]) -> Option<fb::ArticleBlockType> {
    block_stack.last().map(|block| block.block_type)
}

fn append_block(
    block_stack: &mut Vec<ArticleBlock>,
    root_blocks: &mut Vec<ArticleBlock>,
    block: ArticleBlock,
) {
    if let Some(parent) = block_stack.last_mut() {
        parent.children.push(block);
    } else {
        root_blocks.push(block);
    }
}

fn append_inline(
    block_stack: &mut [ArticleBlock],
    inline_stack: &mut Vec<ArticleInline>,
    inline: ArticleInline,
) {
    if let Some(parent) = inline_stack.last_mut() {
        parent.children.push(inline);
    } else if let Some(block) = block_stack.last_mut() {
        block.inlines.push(inline);
    }
}

fn append_text_to_current_block(block_stack: &mut [ArticleBlock], text: &str) {
    if let Some(block) = block_stack.last_mut() {
        block.text
            .get_or_insert_with(String::new)
            .push_str(text);
    }
}

fn parse_text_inlines(text: &str) -> Vec<ArticleInline> {
    let matcher =
        Regex::new(r"(?i)(nostr:)?((?:nprofile|npub|nevent|note|naddr)1[a-z0-9]+)|(^|[\s\(\[])(#[A-Za-z0-9_]+)")
            .expect("valid article inline regex");
    let mut inlines = Vec::new();
    let mut cursor = 0;

    for captures in matcher.captures_iter(text) {
        let Some(matched) = captures.get(0) else {
            continue;
        };
        let mut start = matched.start();
        let end = matched.end();

        if let Some(prefix) = captures.get(3) {
            start = prefix.end();
            if cursor < start {
                push_text_inline(&mut inlines, &text[cursor..start]);
            }
        } else if cursor < start {
            push_text_inline(&mut inlines, &text[cursor..start]);
        }

        if let Some(entity_match) = captures.get(2) {
            let entity = entity_match.as_str();
            let mut inline = article_inline(fb::ArticleInlineType::NostrEntity);
            inline.text = Some(entity.to_string());
            inline.entity = Some(ArticleEntity {
                entity: entity.to_string(),
                id: None,
                relays: Vec::new(),
                author: None,
                kind: None,
            });
            inlines.push(inline);
        } else if let Some(tag_match) = captures.get(4) {
            let tag = tag_match.as_str().trim_start_matches('#');
            let mut inline = article_inline(fb::ArticleInlineType::Hashtag);
            inline.text = Some(format!("#{tag}"));
            inline.tag = Some(tag.to_string());
            inlines.push(inline);
        }
        cursor = end;
    }

    if cursor < text.len() {
        push_text_inline(&mut inlines, &text[cursor..]);
    }

    if inlines.is_empty() {
        push_text_inline(&mut inlines, text);
    }
    inlines
}

fn push_text_inline(inlines: &mut Vec<ArticleInline>, text: &str) {
    if text.is_empty() {
        return;
    }
    let mut inline = article_inline(fb::ArticleInlineType::Text);
    inline.text = Some(text.to_string());
    inlines.push(inline);
}

fn build_article_entity<'a, A: flatbuffers::Allocator + 'a>(
    entity: &ArticleEntity,
    builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
) -> flatbuffers::WIPOffset<fb::ArticleEntity<'a>> {
    let entity_value = builder.create_string(&entity.entity);
    let id = entity.id.as_ref().map(|value| builder.create_string(value));
    let author = entity.author.as_ref().map(|value| builder.create_string(value));
    let relay_offsets: Vec<_> = entity
        .relays
        .iter()
        .map(|relay| builder.create_string(relay))
        .collect();
    let relays = if relay_offsets.is_empty() {
        None
    } else {
        Some(builder.create_vector(&relay_offsets))
    };

    fb::ArticleEntity::create(
        builder,
        &fb::ArticleEntityArgs {
            entity: Some(entity_value),
            id,
            relays,
            author,
            kind: entity.kind.unwrap_or(0),
        },
    )
}

fn build_article_inline<'a, A: flatbuffers::Allocator + 'a>(
    inline: &ArticleInline,
    builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
) -> flatbuffers::WIPOffset<fb::ArticleInline<'a>> {
    let text = inline.text.as_ref().map(|value| builder.create_string(value));
    let url = inline.url.as_ref().map(|value| builder.create_string(value));
    let tag = inline.tag.as_ref().map(|value| builder.create_string(value));
    let entity = inline
        .entity
        .as_ref()
        .map(|value| build_article_entity(value, builder));
    let child_offsets: Vec<_> = inline
        .children
        .iter()
        .map(|child| build_article_inline(child, builder))
        .collect();
    let children = if child_offsets.is_empty() {
        None
    } else {
        Some(builder.create_vector(&child_offsets))
    };

    fb::ArticleInline::create(
        builder,
        &fb::ArticleInlineArgs {
            type_: inline.inline_type,
            text,
            url,
            tag,
            entity,
            children,
        },
    )
}

fn build_article_block<'a, A: flatbuffers::Allocator + 'a>(
    block: &ArticleBlock,
    builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
) -> flatbuffers::WIPOffset<fb::ArticleBlock<'a>> {
    let text = block.text.as_ref().map(|value| builder.create_string(value));
    let url = block.url.as_ref().map(|value| builder.create_string(value));
    let language = block
        .language
        .as_ref()
        .map(|value| builder.create_string(value));
    let inline_offsets: Vec<_> = block
        .inlines
        .iter()
        .map(|inline| build_article_inline(inline, builder))
        .collect();
    let inlines = if inline_offsets.is_empty() {
        None
    } else {
        Some(builder.create_vector(&inline_offsets))
    };
    let child_offsets: Vec<_> = block
        .children
        .iter()
        .map(|child| build_article_block(child, builder))
        .collect();
    let children = if child_offsets.is_empty() {
        None
    } else {
        Some(builder.create_vector(&child_offsets))
    };

    fb::ArticleBlock::create(
        builder,
        &fb::ArticleBlockArgs {
            type_: block.block_type,
            text,
            url,
            language,
            depth: block.depth,
            ordered: block.ordered,
            start: block.start,
            inlines,
            children,
        },
    )
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
