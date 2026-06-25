use crate::generated::nostr::*;
use crate::parser::{Parser, ParserError, Result};
use crate::types::network::Request;
use crate::types::Event;

#[derive(Debug, Clone)]
pub struct BadgeAwardRecipient {
    pub pubkey: String,
    pub relay: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Kind8Parsed {
    pub badge_address: String,
    pub badge_relay: Option<String>,
    pub recipients: Vec<BadgeAwardRecipient>,
    pub content: String,
}

impl Parser {
    pub fn parse_kind_8(&self, event: &Event) -> Result<(Kind8Parsed, Option<Vec<Request>>)> {
        if event.kind != 8 {
            return Err(ParserError::Other("event is not kind 8".to_string()));
        }

        let badge_tag = event
            .tags
            .iter()
            .find(|tag| tag.len() >= 2 && tag[0] == "a")
            .ok_or_else(|| ParserError::MissingField("badge address tag".to_string()))?;

        let badge_address = badge_tag[1].clone();
        if !badge_address.starts_with("30009:") {
            return Err(ParserError::InvalidTag(
                "badge award a tag must reference a kind 30009 badge definition".to_string(),
            ));
        }

        let recipients: Vec<_> = event
            .tags
            .iter()
            .filter(|tag| tag.len() >= 2 && tag[0] == "p")
            .map(|tag| BadgeAwardRecipient {
                pubkey: tag[1].clone(),
                relay: tag.get(2).cloned().filter(|relay| !relay.is_empty()),
            })
            .collect();

        if recipients.is_empty() {
            return Err(ParserError::MissingField(
                "badge award recipient p tag".to_string(),
            ));
        }

        Ok((
            Kind8Parsed {
                badge_address,
                badge_relay: badge_tag.get(2).cloned().filter(|relay| !relay.is_empty()),
                recipients,
                content: event.content.clone(),
            },
            None,
        ))
    }
}

pub fn build_flatbuffer<'a, A: flatbuffers::Allocator + 'a>(
    parsed: &Kind8Parsed,
    builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
) -> Result<flatbuffers::WIPOffset<fb::Kind8Parsed<'a>>> {
    let badge_address = builder.create_string(&parsed.badge_address);
    let badge_relay = parsed
        .badge_relay
        .as_ref()
        .map(|relay| builder.create_string(relay));
    let content = if parsed.content.is_empty() {
        None
    } else {
        Some(builder.create_string(&parsed.content))
    };

    let recipient_offsets: Vec<_> = parsed
        .recipients
        .iter()
        .map(|recipient| {
            let pubkey = builder.create_string(&recipient.pubkey);
            let relay = recipient
                .relay
                .as_ref()
                .map(|relay| builder.create_string(relay));

            fb::BadgeAwardRecipient::create(
                builder,
                &fb::BadgeAwardRecipientArgs {
                    pubkey: Some(pubkey),
                    relay,
                },
            )
        })
        .collect();
    let recipients = builder.create_vector(&recipient_offsets);

    Ok(fb::Kind8Parsed::create(
        builder,
        &fb::Kind8ParsedArgs {
            badge_address: Some(badge_address),
            badge_relay,
            recipients: Some(recipients),
            content,
        },
    ))
}
