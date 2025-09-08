use crate::parsed_event::ParsedData;
use crate::types::network::Request;
use crate::utils::request_deduplication::RequestDeduplicator;
use crate::{parsed_event::ParsedEvent, parser::Parser};
use anyhow::{anyhow, Result};
use nostr::{Event, Kind};
use serde::{Deserialize, Serialize};
use tracing::debug;

// NEW: Imports for FlatBuffers
use crate::generated::nostr::*;
use flatbuffers::FlatBufferBuilder;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Kind6Parsed {
    #[serde(rename = "repostedEvent", skip_serializing_if = "Option::is_none")]
    pub reposted_event: Option<serde_json::Value>,
}

impl Parser {
    pub fn parse_kind_6(&self, event: &Event) -> Result<(Kind6Parsed, Option<Vec<Request>>)> {
        let mut requests = Vec::<Request>::new();
        if event.kind() != Kind::Repost {
            return Err(anyhow!("event is not kind 6"));
        }

        // Add request for the author's metadata
        // requests.push(Request {
        //     kinds: vec![0],
        //     authors: vec![event.pubkey.to_string()],
        //     cache_first: true,
        //     close_on_eose: true,
        //     relays: self
        //         .database
        //         .find_relay_candidates(0, &event.pubkey.to_string(), &false),
        //     ..Default::default()
        // });

        // Find the e tag for the reposted event (should be the last one if multiple)
        let e_tag = match event
            .tags
            .iter()
            .rev()
            .find(|tag| tag.as_vec().first().map(|s| s.as_str()) == Some("e"))
        {
            Some(tag) if tag.as_vec().len() >= 2 => tag,
            _ => return Err(anyhow!("repost must have at least one e tag")),
        };

        let event_id = e_tag.as_vec()[1].clone();

        // Extract relay hint if available
        let mut relay_hint = String::new();
        if e_tag.as_vec().len() >= 3 {
            relay_hint = e_tag.as_vec()[2].clone();
        }

        // Try to parse the reposted event from content
        let mut reposted_event = None;

        if !event.content.is_empty() {
            debug!(
                "Attempting to parse reposted event from content: {}",
                event.content
            );
            match serde_json::from_str::<Event>(&event.content) {
                Ok(parsed_event)
                    if !parsed_event.id.to_string().is_empty()
                        && parsed_event.kind() == Kind::TextNote =>
                {
                    debug!("Parsed event: {:?}", parsed_event);
                    // Parse the event using kind1 parser
                    match self.parse_kind_1(&parsed_event) {
                        Ok((parsed_content, parsed_requests)) => {
                            // Create a ParsedEvent with the parsed content and serialize to JSON
                            let parsed_event_struct = ParsedEvent {
                                event: parsed_event,
                                parsed: Some(ParsedData::Kind1(parsed_content)),
                                relays: vec![],
                                requests: Some(vec![]),
                            };
                            reposted_event = serde_json::to_value(parsed_event_struct).ok();

                            // Add all requests from kind1 parsing
                            if let Some(reqs) = parsed_requests {
                                requests.extend(reqs);
                            }
                        }
                        _ => {}
                    }
                }
                _ => {
                    debug!("Failed to parse reposted event content: {}", event.content);
                    // Try to parse as a different format or structure if needed
                }
            }
        }

        // If we couldn't parse the content or it was empty, request the original event
        if reposted_event.is_none() {
            let mut relays = self
                .database
                .find_relay_candidates(1, &event.pubkey.to_hex(), &false);
            relays.push(relay_hint);

            requests.push(Request {
                ids: vec![event_id],
                cache_first: true,
                close_on_eose: true,
                relays,
                ..Default::default()
            });
        }

        let result = Kind6Parsed { reposted_event };

        // Deduplicate requests using the utility
        let deduplicated_requests = RequestDeduplicator::deduplicate_requests(requests);

        Ok((result, Some(deduplicated_requests)))
    }
}

// NEW: Build the FlatBuffer for Kind6Parsed
pub fn build_flatbuffer<'a, A: flatbuffers::Allocator + 'a>(
    _parsed: &Kind6Parsed,
    builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
) -> Result<flatbuffers::WIPOffset<fb::Kind6Parsed<'a>>> {
    // For now, we'll set reposted_event to None since building a full ParsedEvent
    // FlatBuffer requires complex nested structures (NostrEvent + parsed data)
    // This would need a complete implementation to properly deserialize the JSON
    // and rebuild the FlatBuffer structures
    let reposted_event = None;

    let args = fb::Kind6ParsedArgs { reposted_event };

    let offset = fb::Kind6Parsed::create(builder, &args);

    Ok(offset)
}
