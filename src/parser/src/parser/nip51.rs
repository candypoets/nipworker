use crate::parser::{Parser, ParserError, Result};
use shared::{
    generated::nostr::*,
    types::{network::Request, Event},
}; // brings `fb::...` into scope

use tracing::warn;

/// Coordinate for an "a" tag entry: `kind:pubkey:d` with optional relay(s).
#[derive(Debug, Clone)]
pub struct Coordinate {
    pub kind: u64,
    pub pubkey: String,
    pub d: String,
    pub relays: Vec<String>,
}

/// Unified parsed representation of NIP-51 lists/sets.
/// Covers both 10000- and 30000-range kinds, and compatible custom kinds (e.g., 39089 follow packs).
#[derive(Debug, Clone)]
pub struct ListParsed {
    /// Original event kind (e.g., 10000..19999, 30000..39999, 39089)
    pub list_kind: u16,
    /// PRE identifier from "d" tag (primarily for 30000-range lists)
    pub d: Option<String>,
    /// Optional human-readable metadata
    pub title: Option<String>,
    pub description: Option<String>,
    pub image: Option<String>,
    /// Repeated "t" tags
    pub topics: Vec<String>,
    /// Entries derived from tags
    pub people: Vec<String>, // "p" tags: pubkeys
    pub events: Vec<String>,        // "e" tags: event ids
    pub addresses: Vec<Coordinate>, // "a" tags: coordinates (kind:pubkey:d) + optional relay
}

impl Parser {
    /// Parse NIP-51 lists/sets (RE and PRE).
    ///
    /// Supported kinds:
    /// - 10000..19999 (replaceable lists)
    /// - 30000..39999 (parameterized replaceable lists, utilize "d" tag)
    /// - 39089 (follow packs; treated as list-compatible)
    pub async fn parse_nip51(&self, event: &Event) -> Result<(ListParsed, Option<Vec<Request>>)> {
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
        let mut d = tag_value(&event.tags, "d");
        let mut title = tag_value(&event.tags, "title");
        let mut description =
            tag_value(&event.tags, "description").or_else(|| tag_value(&event.tags, "summary"));
        let mut image = tag_value(&event.tags, "image");
        let mut topics = tag_values(&event.tags, "t");

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

        // Decrypt private content when specified by tags and merge entries parsed like tags
        if let Some(enc) = tag_value(&event.tags, "encryption").map(|s| s.to_ascii_lowercase()) {
            if !event.content.trim().is_empty() {
                // Determine the correct counterparty like kind4 does
                let sender_pubkey = event.pubkey.to_string();
                let decrypted = match enc.as_str() {
                    "nip04" | "nip-04" => self
                        .crypto_client
                        .nip04_decrypt(&sender_pubkey, &event.content)
                        .await
                        .ok(),
                    "nip44" | "nip-44" => self
                        .crypto_client
                        .nip44_decrypt(&sender_pubkey, &event.content)
                        .await
                        .ok(),
                    _ => None,
                };

                if decrypted.is_none() {
                    warn!(
                        "Failed to decrypt list content for kind {} from {}",
                        event.kind,
                        event.pubkey.to_hex()
                    );
                }
                if let Some(plaintext) = decrypted {
                    if let Ok(decrypted_tags) = parse_tag_arrays_json(&plaintext) {
                        for tag in decrypted_tags {
                            if tag.is_empty() {
                                continue;
                            }
                            match tag[0].as_str() {
                                "p" if tag.len() >= 2 => {
                                    people.push(tag[1].clone());
                                }
                                "e" if tag.len() >= 2 => {
                                    events_vec.push(tag[1].clone());
                                }
                                "a" if tag.len() >= 2 => {
                                    if let Some(coord) = parse_coordinate(&tag[1], tag.get(2)) {
                                        addresses.push(coord);
                                    }
                                }
                                "t" if tag.len() >= 2 => {
                                    topics.push(tag[1].clone());
                                }
                                "title" if tag.len() >= 2 => {
                                    if title.is_none() {
                                        title = Some(tag[1].clone());
                                    }
                                }
                                "summary" if tag.len() >= 2 => {
                                    if description.is_none() {
                                        description = Some(tag[1].clone());
                                    }
                                }
                                "description" if tag.len() >= 2 => {
                                    if description.is_none() {
                                        description = Some(tag[1].clone());
                                    }
                                }
                                "image" if tag.len() >= 2 => {
                                    if image.is_none() {
                                        image = Some(tag[1].clone());
                                    }
                                }
                                "d" if tag.len() >= 2 => {
                                    if d.is_none() {
                                        d = Some(tag[1].clone());
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }

        let parsed = ListParsed {
            list_kind: event.kind as u16,
            d,
            title,
            description,
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
    let description = parsed
        .description
        .as_ref()
        .map(|s| builder.create_string(s));
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
        description,
        image,
        topics: topics_vec,
        people: people_vec,
        events: events_vec,
        addresses: addresses_vec,
    };

    Ok(fb::ListParsed::create(builder, &args))
}

// Parse decrypted JSON that mirrors Nostr tags (array of string arrays), e.g.:
// [["p","<pubkey>"],["e","<id>"],["a","<kind:pubkey:d>","<relay>"],["t","topic"],["title","..."], ...]
fn parse_tag_arrays_json(json: &str) -> Result<Vec<Vec<String>>> {
    let mut parser = crate::utils::json::BaseJsonParser::new(json.as_bytes());
    parser.skip_whitespace();
    parser.expect_byte(b'[')?;
    let mut out = Vec::new();

    loop {
        parser.skip_whitespace();
        if parser.pos >= parser.bytes.len() {
            break;
        }
        if parser.peek() == b']' {
            // end of outer array
            parser.pos += 1;
            break;
        }

        // Expect an inner array
        if parser.peek() == b'[' {
            let arr = parse_string_array(&mut parser)?;
            out.push(arr);
        } else {
            // Skip unexpected element
            parser.skip_value()?;
        }

        parser.skip_comma_or_end()?;
    }

    Ok(out)
}

// Parse an array of strings from the current parser position (expects '[')
fn parse_string_array(parser: &mut crate::utils::json::BaseJsonParser) -> Result<Vec<String>> {
    parser.expect_byte(b'[')?;
    let mut arr = Vec::new();

    loop {
        parser.skip_whitespace();
        if parser.pos >= parser.bytes.len() {
            return Err(ParserError::InvalidFormat("Unterminated array".to_string()));
        }
        if parser.peek() == b']' {
            parser.pos += 1;
            break;
        }

        if parser.peek() == b'"' {
            let s = parser.parse_string_unescaped()?;
            arr.push(s);
        } else {
            // Skip non-string values to be tolerant
            parser.skip_value()?;
        }

        parser.skip_comma_or_end()?;
    }

    Ok(arr)
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
