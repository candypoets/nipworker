use crate::parser::{Parser, ParserError, Result};
use crate::{
    generated::nostr::*,
    types::{
        network::Request,
        nostr::{EventId, PublicKey, Template},
        Event,
    },
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
    /// Tags not represented by the typed fields above.
    ///
    /// This includes NIP-51 list-specific entries such as `relay` for kind 30002,
    /// `url`, `emoji`, `server`, custom app tags, and private decrypted tags with
    /// the same shape.
    pub other_tags: Vec<Vec<String>>,
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

        let mut d = None;
        let mut title = None;
        let mut description = None;
        let mut image = None;
        let mut topics = Vec::new();
        let mut people = Vec::new();
        let mut events_vec = Vec::new();
        let mut addresses = Vec::new();
        let mut other_tags = Vec::new();

        process_list_tags(
            &event.tags,
            &mut d,
            &mut title,
            &mut description,
            &mut image,
            &mut topics,
            &mut people,
            &mut events_vec,
            &mut addresses,
            &mut other_tags,
        );

        // Private NIP-51 content is a JSON array shaped like tags.
        // Prefer explicit encryption tags when present, otherwise use the NIP-51
        // backward-compatibility signal: NIP-04 ciphertext contains "?iv=",
        // while current private list content should be NIP-44.
        if !event.content.trim().is_empty() {
            let author = event.pubkey.to_hex();
            let encryption = tag_value(&event.tags, "encryption").map(|s| s.to_ascii_lowercase());
            let plaintext = if let Some(signer) = &self.signer {
                let result = match encryption.as_deref() {
                    Some("nip04") | Some("nip-04") => {
                        signer
                            .nip04_decrypt_between(&author, &author, &event.content)
                            .await
                    }
                    Some("nip44") | Some("nip-44") => {
                        signer
                            .nip44_decrypt_between(&author, &author, &event.content)
                            .await
                    }
                    _ if event.content.contains("?iv=") => {
                        signer
                            .nip04_decrypt_between(&author, &author, &event.content)
                            .await
                    }
                    _ => {
                        signer
                            .nip44_decrypt_between(&author, &author, &event.content)
                            .await
                    }
                };
                match result {
                    Ok(pt) => pt,
                    Err(e) => {
                        warn!(
                            "Failed to decrypt NIP-51 content: {}, treating as plaintext",
                            e
                        );
                        event.content.clone()
                    }
                }
            } else {
                event.content.clone()
            };

            if let Ok(decrypted_tags) = parse_tag_arrays_json(&plaintext) {
                process_list_tags(
                    &decrypted_tags,
                    &mut d,
                    &mut title,
                    &mut description,
                    &mut image,
                    &mut topics,
                    &mut people,
                    &mut events_vec,
                    &mut addresses,
                    &mut other_tags,
                );
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
            other_tags,
        };

        // By default, do not schedule follow-up requests for list parsing.
        Ok((parsed, None))
    }

    /// Prepare (create) a NIP-51 list event, with optional encrypted private content.
    ///
    /// If the template contains an "encryption" tag (nip04 or nip44), the content
    /// will be encrypted using the specified NIP and stored in the .content field.
    /// The public tags remain unencrypted in the tags array.
    ///
    /// # Arguments
    /// * `template` - The event template with:
    ///   - `content`: Plaintext JSON array of private tags (e.g., '[["p","privkey"]]')
    ///   - `tags`: Public tags + optional ["encryption", "nip44"] tag
    ///
    /// # Returns
    /// A signed Event ready for publishing.
    pub async fn prepare_nip51(&self, template: &Template) -> Result<Event> {
        let kind = template.kind;
        let kind_u32 = kind as u32;
        let is_1000x = (10000..20000).contains(&kind_u32);
        let is_3000x = (30000..40000).contains(&kind_u32);
        let is_custom_followpack = kind_u32 == 39089;

        if !is_1000x && !is_3000x && !is_custom_followpack {
            return Err(ParserError::Other(
                "event is not a NIP-51 list kind".to_string(),
            ));
        }

        // Check for encryption tag
        let encryption = tag_value(&template.tags, "encryption").map(|s| s.to_ascii_lowercase());

        let signer = self.signer.as_ref().ok_or_else(|| {
            ParserError::Crypto("encryption not available in parser; signer not configured".into())
        })?;

        let (content, tags) = match encryption.as_deref() {
            Some("nip04") | Some("nip-04") => {
                let own_pubkey = signer
                    .get_public_key()
                    .await
                    .map_err(|e| ParserError::Crypto(format!("get_public_key error: {}", e)))?;
                let encrypted = signer
                    .nip04_encrypt(&own_pubkey, &template.content)
                    .await
                    .map_err(|e| ParserError::Crypto(format!("NIP-04 encrypt error: {}", e)))?;
                (encrypted, template.tags.clone())
            }
            Some("nip44") | Some("nip-44") => {
                let own_pubkey = signer
                    .get_public_key()
                    .await
                    .map_err(|e| ParserError::Crypto(format!("get_public_key error: {}", e)))?;
                let encrypted = signer
                    .nip44_encrypt(&own_pubkey, &template.content)
                    .await
                    .map_err(|e| ParserError::Crypto(format!("NIP-44 encrypt error: {}", e)))?;
                (encrypted, template.tags.clone())
            }
            _ => {
                // No encryption, pass through as-is
                (template.content.clone(), template.tags.clone())
            }
        };

        let new_template = Template::new(kind, content, tags);
        self.sign_template(&new_template).await
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

    let other_tag_offsets: Vec<_> = parsed
        .other_tags
        .iter()
        .filter(|tag| !tag.is_empty())
        .map(|tag| {
            let key = builder.create_string(&tag[0]);
            let values = if tag.len() > 1 {
                let value_offsets: Vec<_> = tag[1..]
                    .iter()
                    .map(|value| builder.create_string(value))
                    .collect();
                Some(builder.create_vector(&value_offsets))
            } else {
                None
            };
            fb::Tag::create(
                builder,
                &fb::TagArgs {
                    key: Some(key),
                    values,
                },
            )
        })
        .collect();

    let other_tags = if other_tag_offsets.is_empty() {
        None
    } else {
        Some(builder.create_vector(&other_tag_offsets))
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
        other_tags,
    };

    Ok(fb::ListParsed::create(builder, &args))
}

#[allow(clippy::too_many_arguments)]
fn process_list_tags(
    tags: &[Vec<String>],
    d: &mut Option<String>,
    title: &mut Option<String>,
    description: &mut Option<String>,
    image: &mut Option<String>,
    topics: &mut Vec<String>,
    people: &mut Vec<String>,
    events_vec: &mut Vec<String>,
    addresses: &mut Vec<Coordinate>,
    other_tags: &mut Vec<Vec<String>>,
) {
    for tag in tags {
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
                    *title = Some(tag[1].clone());
                }
            }
            "summary" | "description" if tag.len() >= 2 => {
                if description.is_none() {
                    *description = Some(tag[1].clone());
                }
            }
            "image" if tag.len() >= 2 => {
                if image.is_none() {
                    *image = Some(tag[1].clone());
                }
            }
            "d" if tag.len() >= 2 => {
                if d.is_none() {
                    *d = Some(tag[1].clone());
                }
            }
            _ => {
                other_tags.push(tag.clone());
            }
        }
    }
}

// Parse decrypted JSON that mirrors Nostr tags (array of string arrays), e.g.:
// [["p","<pubkey>"],["e","<id>"],["a","<kind:pubkey:d>","<relay>"],["t","topic"],["title","..."], ...]
fn parse_tag_arrays_json(json: &str) -> Result<Vec<Vec<String>>> {
    let mut parser = crate::parser_utils::json::BaseJsonParser::new(json.as_bytes());
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
fn parse_string_array(
    parser: &mut crate::parser_utils::json::BaseJsonParser,
) -> Result<Vec<String>> {
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

#[cfg(test)]
mod tests {
    use crate::parser::Parser;
    use crate::types::nostr::{Event, EventId, PublicKey};

    fn event(tags: Vec<Vec<&str>>, content: &str) -> Event {
        Event {
            id: EventId([1; 32]),
            pubkey: PublicKey([2; 32]),
            created_at: 0,
            kind: 30002,
            tags: tags
                .into_iter()
                .map(|tag| tag.into_iter().map(str::to_string).collect())
                .collect(),
            content: content.to_string(),
            sig: "00".to_string(),
        }
    }

    #[tokio::test]
    async fn parses_public_relay_tags_as_other_tags() {
        let parser = Parser::new(None);
        let event = event(
            vec![
                vec!["d", "admin-relays"],
                vec!["title", "Admin relays"],
                vec!["relay", "wss://relay.nuts.cash"],
            ],
            "",
        );

        let (parsed, _) = parser.parse_nip51(&event).await.unwrap();

        assert_eq!(parsed.d.as_deref(), Some("admin-relays"));
        assert_eq!(
            parsed.other_tags,
            vec![vec![
                "relay".to_string(),
                "wss://relay.nuts.cash".to_string()
            ]]
        );
    }

    #[tokio::test]
    async fn merges_private_content_tags_as_same_tag_stream() {
        let parser = Parser::new(None);
        let event = event(
            vec![vec!["d", "admin-relays"]],
            r#"[["relay","wss://private.nuts.cash"],["t","nuts"],["p","aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]]"#,
        );

        let (parsed, _) = parser.parse_nip51(&event).await.unwrap();

        assert_eq!(parsed.topics, vec!["nuts".to_string()]);
        assert_eq!(
            parsed.people,
            vec!["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string()]
        );
        assert_eq!(
            parsed.other_tags,
            vec![vec![
                "relay".to_string(),
                "wss://private.nuts.cash".to_string()
            ]]
        );
    }
}
