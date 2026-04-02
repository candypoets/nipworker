use crate::parser::{Parser, ParserError, Result};
use shared::generated::nostr::*;
use shared::types::{network::Request, nostr::Template, Event};

pub struct Kind1018Parsed {
    pub id: String,
    pub pubkey: String,
    pub poll_event_id: String,
    pub selected_options: Vec<String>,
}

impl Parser {
    pub fn parse_kind_1018(
        &self,
        event: &Event,
    ) -> Result<(Kind1018Parsed, Option<Vec<Request>>)> {
        if event.kind != 1018 {
            return Err(ParserError::Other("event is not kind 1018".to_string()));
        }

        let mut poll_event_id: Option<String> = None;
        let mut selected_options = Vec::new();

        for tag in &event.tags {
            if tag.is_empty() {
                continue;
            }
            match tag[0].as_str() {
                "e" if tag.len() >= 2 => {
                    if poll_event_id.is_none() {
                        poll_event_id = Some(tag[1].clone());
                    }
                }
                "response" if tag.len() >= 2 => {
                    selected_options.push(tag[1].clone());
                }
                _ => {}
            }
        }

        let poll_event_id = poll_event_id.ok_or_else(|| {
            ParserError::Other("poll response must reference a poll via e tag".to_string())
        })?;

        if selected_options.is_empty() {
            return Err(ParserError::Other(
                "poll response must include at least one response tag".to_string(),
            ));
        }

        let parsed = Kind1018Parsed {
            id: event.id.to_hex(),
            pubkey: event.pubkey.to_hex(),
            poll_event_id,
            selected_options,
        };

        Ok((parsed, None))
    }

    pub async fn prepare_kind_1018(&self, template: &Template) -> Result<Event> {
        if template.kind != 1018 {
            return Err(ParserError::Other("event is not kind 1018".to_string()));
        }

        // Validate required tags
        let mut has_e_tag = false;
        let mut has_response = false;

        for tag in &template.tags {
            if tag.is_empty() {
                continue;
            }
            match tag[0].as_str() {
                "e" => has_e_tag = true,
                "response" => has_response = true,
                _ => {}
            }
        }

        if !has_e_tag {
            return Err(ParserError::Other(
                "kind 1018 poll response must reference a poll via e tag".to_string(),
            ));
        }

        if !has_response {
            return Err(ParserError::Other(
                "kind 1018 poll response must include at least one response tag".to_string(),
            ));
        }

        let template_json = template.to_json();
        let signed_event_json = self
            .crypto_client
            .sign_event(template_json)
            .await
            .map_err(|e| ParserError::Crypto(format!("Signer error: {}", e)))?;

        let new_event = Event::from_json(&signed_event_json)?;
        Ok(new_event)
    }
}

pub fn build_flatbuffer<'a, A: flatbuffers::Allocator + 'a>(
    parsed: &Kind1018Parsed,
    builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
) -> Result<flatbuffers::WIPOffset<fb::Kind1018Parsed<'a>>> {
    let id = builder.create_string(&parsed.id);
    let pubkey = builder.create_string(&parsed.pubkey);
    let poll_event_id = builder.create_string(&parsed.poll_event_id);

    // Build selected options vector
    let selected_offsets: Vec<_> = parsed
        .selected_options
        .iter()
        .map(|opt| builder.create_string(opt))
        .collect();
    let selected_vector = builder.create_vector(&selected_offsets);

    let args = fb::Kind1018ParsedArgs {
        id: Some(id),
        pubkey: Some(pubkey),
        poll_event_id: Some(poll_event_id),
        selected_options: Some(selected_vector),
    };

    let offset = fb::Kind1018Parsed::create(builder, &args);
    Ok(offset)
}
