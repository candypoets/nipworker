use crate::parser::{Parser, ParserError, Result};
use shared::{
    generated::nostr::*,
    types::{network::Request, Event},
}; // brings `fb::...` into scope

/// Participant parsed from a "p" tag:
/// ["p", "<pubkey>", "<relay?>", "<role?>", "<proof?>"]
#[derive(Debug, Clone)]
pub struct PreParticipant {
    pub pubkey: String,
    pub relay: Option<String>,
    pub role: Option<String>,
    pub proof: Option<String>,
}

/// Event reference parsed from an "e" tag:
/// ["e", "<id>", "<relay?>", "<marker?>"]
#[derive(Debug, Clone)]
pub struct PreRefEvent {
    pub id: String,
    pub relay: Option<String>,
    pub marker: Option<String>,
}

/// Generic PRE (Parameterized Replaceable Event) parsed structure for non-NIP-51 30k kinds.
/// This is a tag-only parser (no content decryption). It normalizes common fields across:
/// - NIP-53 (30311/30312/30313): live activities, spaces, sessions
/// - NIP-58 (30009/30008): badge definition and profile badges
/// - NIP-52 (31922/31923/31924/31925): calendar events, calendars, RSVP
/// - NIP-54 (30818/30819): wiki article and redirects
/// - NIP-78 (30078): arbitrary app data (tags only)
#[derive(Debug, Clone)]
pub struct PreGenericParsed {
    pub kind: u16,
    pub d: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub image: Option<String>,

    // Live/media/service fields
    pub streaming: Option<String>,
    pub recording: Option<String>,
    pub service: Option<String>,
    pub endpoint: Option<String>,
    pub room: Option<String>,

    // Status/timing
    pub status: Option<String>,
    pub starts: Option<u64>,
    pub ends: Option<u64>,

    // Counters / pinned
    pub current_participants: Option<u64>,
    pub total_participants: Option<u64>,
    pub pinned: Option<String>,

    // Collections
    pub topics: Vec<String>, // "t"
    pub links: Vec<String>,  // "r"
    pub relays: Vec<String>, // from "relay" and "relays" tags

    // References
    pub participants: Vec<PreParticipant>,         // from "p"
    pub events: Vec<PreRefEvent>,                  // from "e"
    pub addresses: Vec<crate::parser::Coordinate>, // from "a"
}

impl Parser {
    /// Parse a generic PRE (non-NIP-51) event from the 30k range.
    /// Tag-only extraction, no content decryption.
    pub fn parse_pre_generic(
        &self,
        event: &Event,
    ) -> Result<(PreGenericParsed, Option<Vec<Request>>)> {
        // Only accept 30k range; dispatcher should avoid NIP-51 sets here
        if !(30000..40000).contains(&event.kind) {
            return Err(ParserError::Other(
                "event is not a 30k PRE kind".to_string(),
            ));
        }

        // Core metadata
        let d = tag_value(&event.tags, "d");
        let title = tag_value(&event.tags, "title");
        let description =
            tag_value(&event.tags, "description").or_else(|| tag_value(&event.tags, "summary"));
        let image = tag_value(&event.tags, "image");

        // Media / service / room
        let streaming = tag_value(&event.tags, "streaming");
        let recording = tag_value(&event.tags, "recording");
        let service = tag_value(&event.tags, "service");
        let endpoint = tag_value(&event.tags, "endpoint");
        let room = tag_value(&event.tags, "room");

        // Status and timing
        let status = tag_value(&event.tags, "status");
        let starts = tag_value(&event.tags, "starts").and_then(|s| s.parse::<u64>().ok());
        let ends = tag_value(&event.tags, "ends").and_then(|s| s.parse::<u64>().ok());

        // Counters and pinned
        let current_participants =
            tag_value(&event.tags, "current_participants").and_then(|s| s.parse::<u64>().ok());
        let total_participants =
            tag_value(&event.tags, "total_participants").and_then(|s| s.parse::<u64>().ok());
        let pinned = tag_value(&event.tags, "pinned");

        // Collections
        let mut topics = tag_values(&event.tags, "t");
        let mut links = tag_values(&event.tags, "r");

        // Relays can appear as ["relay", "<url>"] or ["relays", "<url1>", "<url2>", ...]
        let mut relays = Vec::new();
        for tag in &event.tags {
            if tag.is_empty() {
                continue;
            }
            match tag[0].as_str() {
                "relay" if tag.len() >= 2 => relays.push(tag[1].clone()),
                "relays" if tag.len() >= 2 => {
                    for v in tag.iter().skip(1) {
                        if !v.is_empty() {
                            relays.push(v.clone());
                        }
                    }
                }
                _ => {}
            }
        }

        // References
        let mut participants = Vec::new();
        let mut events_vec = Vec::new();
        let mut addresses = Vec::new();

        for tag in &event.tags {
            if tag.is_empty() {
                continue;
            }
            match tag[0].as_str() {
                // ["p","<pubkey>","<relay?>","<role?>","<proof?>"]
                "p" if tag.len() >= 2 => participants.push(PreParticipant {
                    pubkey: tag[1].clone(),
                    relay: tag.get(2).cloned().filter(|s| !s.is_empty()),
                    role: tag.get(3).cloned().filter(|s| !s.is_empty()),
                    proof: tag.get(4).cloned().filter(|s| !s.is_empty()),
                }),
                // ["e","<id>","<relay?>","<marker?>"]
                "e" if tag.len() >= 2 => events_vec.push(PreRefEvent {
                    id: tag[1].clone(),
                    relay: tag.get(2).cloned().filter(|s| !s.is_empty()),
                    marker: tag.get(3).cloned().filter(|s| !s.is_empty()),
                }),
                // ["a","<kind:pubkey:d>","<relay?>", ...]
                "a" if tag.len() >= 2 => {
                    if let Some(coord) = parse_coordinate(&tag[1], tag.get(2)) {
                        addresses.push(coord);
                    }
                }
                "t" if tag.len() >= 2 => topics.push(tag[1].clone()),
                "r" if tag.len() >= 2 => links.push(tag[1].clone()),
                _ => {}
            }
        }

        let parsed = PreGenericParsed {
            kind: event.kind as u16,
            d,
            title,
            description,
            image,
            streaming,
            recording,
            service,
            endpoint,
            room,
            status,
            starts,
            ends,
            current_participants,
            total_participants,
            pinned,
            topics,
            links,
            relays,
            participants,
            events: events_vec,
            addresses,
        };

        Ok((parsed, None))
    }
}

/// Build the FlatBuffer for `PreGenericParsed`.
pub fn build_flatbuffer<'a, A: flatbuffers::Allocator + 'a>(
    parsed: &PreGenericParsed,
    builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
) -> Result<flatbuffers::WIPOffset<fb::PreGenericParsed<'a>>> {
    // Optionals as strings
    let d = parsed.d.as_ref().map(|s| builder.create_string(s));
    let title = parsed.title.as_ref().map(|s| builder.create_string(s));
    let description = parsed
        .description
        .as_ref()
        .map(|s| builder.create_string(s));
    let image = parsed.image.as_ref().map(|s| builder.create_string(s));
    let streaming = parsed.streaming.as_ref().map(|s| builder.create_string(s));
    let recording = parsed.recording.as_ref().map(|s| builder.create_string(s));
    let service = parsed.service.as_ref().map(|s| builder.create_string(s));
    let endpoint = parsed.endpoint.as_ref().map(|s| builder.create_string(s));
    let room = parsed.room.as_ref().map(|s| builder.create_string(s));
    let status = parsed.status.as_ref().map(|s| builder.create_string(s));
    let pinned = parsed.pinned.as_ref().map(|s| builder.create_string(s));

    // Vectors
    let topics = if parsed.topics.is_empty() {
        None
    } else {
        let offs: Vec<_> = parsed
            .topics
            .iter()
            .map(|t| builder.create_string(t))
            .collect();
        Some(builder.create_vector(&offs))
    };

    let links = if parsed.links.is_empty() {
        None
    } else {
        let offs: Vec<_> = parsed
            .links
            .iter()
            .map(|t| builder.create_string(t))
            .collect();
        Some(builder.create_vector(&offs))
    };

    let relays = if parsed.relays.is_empty() {
        None
    } else {
        let offs: Vec<_> = parsed
            .relays
            .iter()
            .map(|r| builder.create_string(r))
            .collect();
        Some(builder.create_vector(&offs))
    };

    let participants = if parsed.participants.is_empty() {
        None
    } else {
        let offs: Vec<_> = parsed
            .participants
            .iter()
            .map(|p| {
                let pubkey = builder.create_string(&p.pubkey);
                let relay = p.relay.as_ref().map(|s| builder.create_string(s));
                let role = p.role.as_ref().map(|s| builder.create_string(s));
                let proof = p.proof.as_ref().map(|s| builder.create_string(s));
                let args = fb::PreParticipantArgs {
                    pubkey: Some(pubkey),
                    relay,
                    role,
                    proof,
                };
                fb::PreParticipant::create(builder, &args)
            })
            .collect();
        Some(builder.create_vector(&offs))
    };

    let events = if parsed.events.is_empty() {
        None
    } else {
        let offs: Vec<_> = parsed
            .events
            .iter()
            .map(|e| {
                let id = builder.create_string(&e.id);
                let relay = e.relay.as_ref().map(|s| builder.create_string(s));
                let marker = e.marker.as_ref().map(|s| builder.create_string(s));
                let args = fb::PreRefEventArgs {
                    id: Some(id),
                    relay,
                    marker,
                };
                fb::PreRefEvent::create(builder, &args)
            })
            .collect();
        Some(builder.create_vector(&offs))
    };

    let addresses = if parsed.addresses.is_empty() {
        None
    } else {
        let offs: Vec<_> = parsed
            .addresses
            .iter()
            .map(|a| {
                let pubkey = builder.create_string(&a.pubkey);
                let d = builder.create_string(&a.d);
                let relays = if a.relays.is_empty() {
                    None
                } else {
                    let roffs: Vec<_> = a.relays.iter().map(|r| builder.create_string(r)).collect();
                    Some(builder.create_vector(&roffs))
                };
                let args = fb::CoordinateArgs {
                    kind: a.kind,
                    pubkey: Some(pubkey),
                    d: Some(d),
                    relays,
                };
                fb::Coordinate::create(builder, &args)
            })
            .collect();
        Some(builder.create_vector(&offs))
    };

    let args = fb::PreGenericParsedArgs {
        kind: parsed.kind,
        d,
        title,
        description,
        image,
        streaming,
        recording,
        service,
        endpoint,
        room,
        status,
        starts: parsed.starts.unwrap_or(0),
        ends: parsed.ends.unwrap_or(0),
        current_participants: parsed.current_participants.unwrap_or(0),
        total_participants: parsed.total_participants.unwrap_or(0),
        pinned,
        topics,
        links,
        relays,
        participants,
        events,
        addresses,
    };

    Ok(fb::PreGenericParsed::create(builder, &args))
}

// ----------------- Helpers -----------------

fn tag_value(tags: &[Vec<String>], key: &str) -> Option<String> {
    tags.iter()
        .find_map(|t| (t.len() >= 2 && t[0] == key).then(|| t[1].clone()))
}

fn tag_values(tags: &[Vec<String>], key: &str) -> Vec<String> {
    tags.iter()
        .filter_map(|t| (t.len() >= 2 && t[0] == key).then(|| t[1].clone()))
        .collect()
}

fn parse_coordinate(coord: &str, relay_opt: Option<&String>) -> Option<crate::parser::Coordinate> {
    // "<kind>:<pubkey>:<d>"
    let mut parts = coord.splitn(3, ':');
    let kind = parts.next()?.parse::<u64>().ok()?;
    let pubkey = parts.next()?.to_string();
    let d = parts.next()?.to_string();

    let relays = relay_opt
        .filter(|s| !s.is_empty())
        .map(|s| vec![s.clone()])
        .unwrap_or_default();

    Some(crate::parser::Coordinate {
        kind,
        pubkey,
        d,
        relays,
    })
}
