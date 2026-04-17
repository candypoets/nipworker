use crate::parser::{Parser, ParserError, Result};
use crate::parser::content::{parse_content, ContentParser, serialize_content_data, ContentBlock};
use crate::generated::nostr::*;
use crate::types::{network::Request, nostr::{Template, EventId, PublicKey}, Event};

pub struct PollOption {
    pub id: String,
    pub label: String,
}

pub struct Kind1068Parsed {
    pub id: String,
    pub pubkey: String,
    pub question: String,
    pub content_blocks: Vec<ContentBlock>,
    pub options: Vec<PollOption>,
    pub poll_type: PollType,
    pub ends_at: u64,
    pub relay_urls: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollType {
    SingleChoice,
    MultipleChoice,
}

impl PollType {
    fn to_fb(&self) -> fb::PollType {
        match self {
            PollType::SingleChoice => fb::PollType::SingleChoice,
            PollType::MultipleChoice => fb::PollType::MultipleChoice,
        }
    }
}

impl Parser {
    pub fn parse_kind_1068(
        &self,
        event: &Event,
    ) -> Result<(Kind1068Parsed, Option<Vec<Request>>)> {
        if event.kind != 1068 {
            return Err(ParserError::Other("event is not kind 1068".to_string()));
        }

        let mut options = Vec::new();
        let mut relay_urls = Vec::new();
        let mut poll_type = PollType::SingleChoice;
        let mut ends_at: u64 = 0;

        for tag in &event.tags {
            if tag.is_empty() {
                continue;
            }
            match tag[0].as_str() {
                "option" if tag.len() >= 3 => {
                    options.push(PollOption {
                        id: tag[1].clone(),
                        label: tag[2].clone(),
                    });
                }
                "relay" if tag.len() >= 2 => {
                    relay_urls.push(tag[1].clone());
                }
                "polltype" if tag.len() >= 2 => {
                    poll_type = match tag[1].as_str() {
                        "multiplechoice" => PollType::MultipleChoice,
                        _ => PollType::SingleChoice,
                    };
                }
                "endsAt" if tag.len() >= 2 => {
                    if let Ok(ts) = tag[1].parse::<u64>() {
                        ends_at = ts;
                    }
                }
                _ => {}
            }
        }

        if options.is_empty() {
            return Err(ParserError::Other(
                "poll must have at least one option tag".to_string(),
            ));
        }

        let parsed = Kind1068Parsed {
            id: event.id.to_hex(),
            pubkey: event.pubkey.to_hex(),
            question: event.content.clone(),
            content_blocks: {
                let emoji_tags: Vec<Vec<String>> = event
                    .tags
                    .iter()
                    .filter(|tag| tag.len() >= 3 && tag[0] == "emoji")
                    .cloned()
                    .collect();
                parse_content(&event.content, &emoji_tags).unwrap_or_default()
            },
            options,
            poll_type,
            ends_at,
            relay_urls,
        };

        Ok((parsed, None))
    }

    pub async fn prepare_kind_1068(&self, template: &Template) -> Result<Event> {
        if template.kind != 1068 {
            return Err(ParserError::Other("event is not kind 1068".to_string()));
        }

        // Validate required tags
        let mut has_option = false;
        for tag in &template.tags {
            if !tag.is_empty() && tag[0] == "option" {
                has_option = true;
                break;
            }
        }

        if !has_option {
            return Err(ParserError::Other(
                "kind 1068 poll must include at least one option tag".to_string(),
            ));
        }

        let new_event = Event {
            id: EventId([0u8; 32]),
            pubkey: PublicKey([0u8; 32]),
            created_at: template.created_at,
            kind: template.kind,
            tags: template.tags.clone(),
            content: template.content.clone(),
            sig: String::new(),
        };
        Ok(new_event)
    }
}

pub fn build_flatbuffer<'a, A: flatbuffers::Allocator + 'a>(
    parsed: &Kind1068Parsed,
    builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
) -> Result<flatbuffers::WIPOffset<fb::Kind1068Parsed<'a>>> {
    let id = builder.create_string(&parsed.id);
    let pubkey = builder.create_string(&parsed.pubkey);
    let question = builder.create_string(&parsed.question);

    // Build content blocks vector
    let mut content_blocks_offsets = Vec::new();
    for block in &parsed.content_blocks {
        let block_type = builder.create_string(&block.block_type);
        let text = builder.create_string(&block.text);
        let (data_type, data) = match &block.data {
            Some(d) => serialize_content_data(builder, d),
            None => (fb::ContentData::NONE, None),
        };

        let content_block_args = fb::ContentBlockArgs {
            type_: Some(block_type),
            text: Some(text),
            data_type,
            data,
        };
        let content_block_offset = fb::ContentBlock::create(builder, &content_block_args);
        content_blocks_offsets.push(content_block_offset);
    }
    let content_blocks_vector = if content_blocks_offsets.is_empty() {
        None
    } else {
        Some(builder.create_vector(&content_blocks_offsets))
    };

    // Build options vector
    let mut option_offsets = Vec::new();
    for opt in &parsed.options {
        let opt_id = builder.create_string(&opt.id);
        let opt_label = builder.create_string(&opt.label);
        let args = fb::PollOptionArgs {
            id: Some(opt_id),
            label: Some(opt_label),
        };
        option_offsets.push(fb::PollOption::create(builder, &args));
    }
    let options_vector = builder.create_vector(&option_offsets);

    // Build relay URLs vector
    let relays_offset = if parsed.relay_urls.is_empty() {
        None
    } else {
        let offsets: Vec<_> = parsed
            .relay_urls
            .iter()
            .map(|r| builder.create_string(r))
            .collect();
        Some(builder.create_vector(&offsets))
    };

    let poll_type_fb = parsed.poll_type.to_fb();

    let args = fb::Kind1068ParsedArgs {
        id: Some(id),
        pubkey: Some(pubkey),
        question: Some(question),
        content_blocks: content_blocks_vector,
        options: Some(options_vector),
        poll_type: poll_type_fb,
        ends_at: parsed.ends_at,
        relay_urls: relays_offset,
    };

    let offset = fb::Kind1068Parsed::create(builder, &args);
    Ok(offset)
}
