use crate::parser::Parser;
use crate::parser::{ParserError, Result};
use crate::types::network::Request;
use crate::types::nostr::Event;

// NEW: Imports for FlatBuffers
use crate::generated::nostr::*;
use crate::utils::relay::RelayUtils;
use flatbuffers::FlatBufferBuilder;

pub struct RelayInfo {
    pub url: String,
    pub read: bool,
    pub write: bool,
}

pub type Kind10002Parsed = Vec<RelayInfo>;

impl Parser {
    pub fn parse_kind_10002(
        &self,
        event: &Event,
    ) -> Result<(Kind10002Parsed, Option<Vec<Request>>)> {
        if event.kind != 10002 {
            return Err(ParserError::Other("event is not kind 10002".to_string()));
        }

        let mut relays = Vec::new();

        // Extract relay info from the r tags
        for tag in &event.tags {
            if tag.len() >= 2 && tag[0] == "r" && !tag[1].is_empty() {
                let url = RelayUtils::normalize_url(&tag[1]);
                if url.is_empty() {
                    continue;
                }

                let marker = if tag.len() >= 3 {
                    tag[2].to_lowercase()
                } else {
                    String::new()
                };

                // If no marker is provided, the relay is used for both read and write
                // If a marker is provided, it should be either "read", "write", or both
                let relay = RelayInfo {
                    url: url,
                    read: marker.is_empty() || marker == "read",
                    write: marker.is_empty() || marker == "write",
                };

                relays.push(relay);
            }
        }

        // Deduplicate relays by URL
        let mut unique_relays = std::collections::HashMap::new();
        for relay in relays {
            unique_relays.insert(relay.url.clone(), relay);
        }

        // Convert map to vec
        let result: Kind10002Parsed = unique_relays.into_values().collect();

        Ok((result, None))
    }
}

// NEW: Build the FlatBuffer for Kind10002Parsed
pub fn build_flatbuffer<'a, A: flatbuffers::Allocator + 'a>(
    parsed: &Kind10002Parsed,
    builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
) -> Result<flatbuffers::WIPOffset<fb::Kind10002Parsed<'a>>> {
    // Build relay info vector
    let mut relay_info_offsets = Vec::new();
    for relay in parsed {
        let url = builder.create_string(&relay.url);

        let relay_info_args = fb::RelayInfoArgs {
            url: Some(url),
            read: relay.read,
            write: relay.write,
        };
        let relay_info_offset = fb::RelayInfo::create(builder, &relay_info_args);
        relay_info_offsets.push(relay_info_offset);
    }
    let relay_info_vector = builder.create_vector(&relay_info_offsets);

    let args = fb::Kind10002ParsedArgs {
        relays: Some(relay_info_vector),
    };

    let offset = fb::Kind10002Parsed::create(builder, &args);

    Ok(offset)
}

fn normalize_relay_url(url: &str) -> String {
    let url = url.trim();
    if url.is_empty() {
        return String::new();
    }

    // Basic URL normalization - could use crate::types::nostr::Url::normalize if available
    if url.starts_with("wss://") || url.starts_with("ws://") {
        url.to_string()
    } else if url.starts_with("//") {
        format!("wss:{}", url)
    } else {
        format!("wss://{}", url)
    }
}
