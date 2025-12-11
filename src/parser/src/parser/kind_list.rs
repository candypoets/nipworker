use crate::generated::nostr::*; // brings `fb::...` into scope
use crate::parser::{Parser, ParserError, Result};
use crate::types::network::Request;
use crate::types::nostr::Event;

/// Coordinate for an "a" tag entry: `kind:pubkey:d` with optional relay(s).
pub struct Coordinate {
    pub kind: u64,
    pub pubkey: String,
    pub d: String,
    pub relays: Vec<String>,
}

/// Unified parsed representation of NIP-51 lists/sets.
/// Covers both 10000- and 30000-range kinds, and compatible custom kinds (e.g., 39089 follow packs).
pub struct ListParsed {
    /// Original event kind (e.g., 10000..19999, 30000..39999, 39089)
    pub list_kind: u16,
    /// PRE identifier from "d" tag (primarily for 30000-range lists)
    pub d: Option<String>,
    /// Optional human-readable metadata
    pub title: Option<String>,
    pub summary: Option<String>, // or "description"
    pub image: Option<String>,
    /// Repeated "t" tags
    pub topics: Vec<String>,
    /// Entries derived from tags
    pub people: Vec<String>, // "p" tags: pubkeys
    pub events: Vec<String>,        // "e" tags: event ids
    pub addresses: Vec<Coordinate>, // "a" tags: coordinates (kind:pubkey:d) + optional relay
}

impl Parser {
    /// Parse a generic NIP-51 list/set.
    ///
    /// Supported kinds:
    /// - 10000..19999 (replaceable lists)
    /// - 30000..39999 (parameterized replaceable lists, utilize "d" tag)
    /// - 39089 (follow packs; treated as list-compatible)
    pub fn parse_kind_list(&self, event: &Event) -> Result<(ListParsed, Option<Vec<Request>>)> {
        let kind_u32 = event.kind as u32;
        let is_1000x = (10000..20000).contains(&kind_u32);
        let is_3000x = (30000..40000).contains(&kind_u32);
        let is_custom_followpack = kind_u32 == 39089;

        if !is_1000x && !is_3000x && !is_custom_followpack {
            return Err(ParserError::Other(
                "event is not a NIP-51 list kind".to_string(),
            ));
        }

        // Metadata
        let d = tag_value(&event.tags, "d");
        let title = tag_value(&event.tags, "title");
        let summary =
            tag_value(&event.tags, "summary").or_else(|| tag_value(&event.tags, "description"));
        let image = tag_value(&event.tags, "image");
        let topics = tag_values(&event.tags, "t");

        // Entries
        let mut people = Vec::new();
        let mut events_vec = Vec::new();
        let mut addresses = Vec::new();

        for tag in &event.tags {
            if tag.len() < 2 {
                continue;
            }
            match tag[0].as_str() {
                "p" => {
                    // ["p", "<pubkey>", <relay?>, ...]
                    people.push(tag[1].clone());
                }
                "e" => {
                    // ["e", "<event_id>", <relay?>, ...]
                    events_vec.push(tag[1].clone());
                }
                "a" => {
                    // ["a", "kind:pubkey:d", <relay?>, ...]
                    if let Some(coord) = parse_coordinate(&tag[1], tag.get(2)) {
                        addresses.push(coord);
                    }
                }
                _ => {}
            }
        }

        let parsed = ListParsed {
            list_kind: event.kind as u16,
            d,
            title,
            summary,
            image,
            topics,
            people,
            events: events_vec,
            addresses,
        };

        // By default, do not schedule follow-up requests for list parsing.
        Ok((parsed, None))
    }
}

/// Build the FlatBuffer for `ListParsed`.
pub fn build_flatbuffer<'a, A: flatbuffers::Allocator + 'a>(
    parsed: &ListParsed,
    builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
) -> Result<flatbuffers::WIPOffset<fb::ListParsed<'a>>> {
    // Optional strings
    let d = parsed.d.as_ref().map(|s| builder.create_string(s));
    let title = parsed.title.as_ref().map(|s| builder.create_string(s));
    let summary = parsed.summary.as_ref().map(|s| builder.create_string(s));
    let image = parsed.image.as_ref().map(|s| builder.create_string(s));

    // topics
    let topics_vec = if parsed.topics.is_empty() {
        None
    } else {
        let offs: Vec<_> = parsed
            .topics
            .iter()
            .map(|t| builder.create_string(t))
            .collect();
        Some(builder.create_vector(&offs))
    };

    // people
    let people_vec = if parsed.people.is_empty() {
        None
    } else {
        let offs: Vec<_> = parsed
            .people
            .iter()
            .map(|p| builder.create_string(p))
            .collect();
        Some(builder.create_vector(&offs))
    };

    // events
    let events_vec = if parsed.events.is_empty() {
        None
    } else {
        let offs: Vec<_> = parsed
            .events
            .iter()
            .map(|e| builder.create_string(e))
            .collect();
        Some(builder.create_vector(&offs))
    };

    // addresses (Coordinate)
    let addr_offs: Vec<_> = parsed
        .addresses
        .iter()
        .map(|a| {
            let pubkey = builder.create_string(&a.pubkey);
            let d_str = builder.create_string(&a.d);
            let relays = if a.relays.is_empty() {
                None
            } else {
                let relay_offs: Vec<_> =
                    a.relays.iter().map(|r| builder.create_string(r)).collect();
                Some(builder.create_vector(&relay_offs))
            };
            let args = fb::CoordinateArgs {
                kind: a.kind,
                pubkey: Some(pubkey),
                d: Some(d_str),
                relays,
            };
            fb::Coordinate::create(builder, &args)
        })
        .collect();

    let addresses_vec = if addr_offs.is_empty() {
        None
    } else {
        Some(builder.create_vector(&addr_offs))
    };

    let args = fb::ListParsedArgs {
        list_kind: parsed.list_kind,
        d,
        title,
        summary,
        image,
        topics: topics_vec,
        people: people_vec,
        events: events_vec,
        addresses: addresses_vec,
    };

    Ok(fb::ListParsed::create(builder, &args))
}

// ------------- Helpers -------------

fn parse_coordinate(coord: &str, relay_opt: Option<&String>) -> Option<Coordinate> {
    // Expected format: "<kind>:<pubkey>:<d>"
    let mut parts = coord.splitn(3, ':');
    let kind = parts.next()?.parse::<u64>().ok()?;
    let pubkey = parts.next()?.to_string();
    let d = parts.next()?.to_string();

    let relays = relay_opt
        .filter(|s| !s.is_empty())
        .map(|s| vec![s.clone()])
        .unwrap_or_default();

    Some(Coordinate {
        kind,
        pubkey,
        d,
        relays,
    })
}

fn tag_value(tags: &[Vec<String>], key: &str) -> Option<String> {
    tags.iter()
        .find_map(|t| (t.len() >= 2 && t[0] == key).then(|| t[1].clone()))
}

fn tag_values(tags: &[Vec<String>], key: &str) -> Vec<String> {
    tags.iter()
        .filter_map(|t| (t.len() >= 2 && t[0] == key).then(|| t[1].clone()))
        .collect()
}
