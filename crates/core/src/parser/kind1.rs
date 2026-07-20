use crate::parser::{
    content::{
        enrich_media_with_imeta, serialize_content_data, ContentBlock, ContentParser, ImetaData,
    },
    Parser,
};
use crate::parser::{ParserError, Result};
use crate::parser_utils::request_deduplication::RequestDeduplicator;

use crate::{
    generated::nostr::*,
    types::{network::Request, nostr, Event},
};
use rustc_hash::FxHashMap;
use std::sync::LazyLock;

// Static mention/reference regexes, compiled once instead of per event
static PROFILE_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?:nostr:)?(npub1[a-z0-9]+)").unwrap());
static NPROFILE_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?:nostr:)?(nprofile1[a-z0-9]+)").unwrap());
static NOTE_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?:nostr:)?(note1[a-z0-9]+)").unwrap());
static NEVENT_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?:nostr:)?(nevent1[a-z0-9]+)").unwrap());
static NADDR_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?:nostr:)?(naddr1[a-z0-9]+)").unwrap());

pub struct ProfilePointer {
    pub public_key: String,
    pub relays: Vec<String>,
}

pub struct EventPointer {
    pub id: String,
    pub relays: Vec<String>,
    pub author: Option<String>,
    pub kind: Option<u64>,
}

pub struct AddressPointer {
    pub kind: u64,
    pub pubkey: String,
    pub d: String,
    pub relays: Vec<String>,
}

pub struct Kind1Parsed {
    pub parsed_content: Vec<ContentBlock>,
    pub shortened_content: Vec<ContentBlock>,
    pub profile_mentions: Vec<ProfilePointer>,
    pub event_refs: Vec<EventPointer>,
    pub address_refs: Vec<AddressPointer>,
    pub reply: Option<EventPointer>,
    pub root: Option<EventPointer>,
}

fn should_use_shortened_content(
    parsed_blocks: &[ContentBlock],
    shortened_blocks: &[ContentBlock],
) -> bool {
    !shortened_blocks.is_empty() && shortened_blocks != parsed_blocks
}

fn relay_hints_for_tag_value(tags: &[Vec<String>], tag_names: &[&str], value: &str) -> Vec<String> {
    let mut relays = Vec::new();
    for tag in tags {
        if tag.len() >= 3 && tag_names.contains(&tag[0].as_str()) && tag[1] == value {
            push_unique_relay(&mut relays, &tag[2]);
        }
    }
    relays
}

fn merge_relays(mut primary: Vec<String>, fallback: Vec<String>) -> Vec<String> {
    for relay in fallback {
        push_unique_relay(&mut primary, &relay);
    }
    primary
}

fn push_unique_relay(relays: &mut Vec<String>, relay: &str) {
    if !relay.is_empty() && !relays.iter().any(|existing| existing == relay) {
        relays.push(relay.to_string());
    }
}

impl Parser {
    pub fn parse_kind_1(&self, event: &Event) -> Result<(Kind1Parsed, Option<Vec<Request>>)> {
        if event.kind != 1 {
            return Err(ParserError::Other("event is not kind 1".to_string()));
        }

        let mut requests = Vec::new();
        let mut parsed = Kind1Parsed {
            parsed_content: Vec::new(),
            shortened_content: Vec::new(),
            profile_mentions: Vec::new(),
            event_refs: Vec::new(),
            address_refs: Vec::new(),
            reply: None,
            root: None,
        };

        // Request profile information for the author
        // requests.push(Request {
        //     authors: vec![event.pubkey.to_hex()],
        //     kinds: vec![0],
        //     relays: self
        //         .database
        //         .find_relay_candidates(0, &event.pubkey.to_hex(), &false),
        //     close_on_eose: true,
        //     cache_first: true,
        //     ..Default::default()
        // });

        // Request relay list for the author
        // requests.push(Request {
        //     authors: vec![event.pubkey.to_hex()],
        //     kinds: vec![10002],
        //     relays: self
        //         .database
        //         .find_relay_candidates(10002, &event.pubkey.to_hex(), &false),
        //     close_on_eose: true,
        //     cache_first: true,
        //     ..Default::default()
        // });

        // Parse references using NIP-27 (nostr: URIs and bech32 entities)
        // For now, we'll parse them manually from content
        parsed.profile_mentions =
            self.extract_profile_mentions(&event.content, &event.tags, &mut requests);
        parsed.event_refs = self.extract_event_refs(&event.content, &event.tags, &mut requests);
        parsed.address_refs = self.extract_address_refs(&event.content, &mut requests);

        // Extract reply and root using NIP-10
        parsed.reply = self.get_immediate_parent(&event.tags);
        if let Some(ref reply) = parsed.reply {
            requests.push(Request {
                ids: vec![reply.id.clone()],
                limit: Some(3), // increase the limit to provide with a bigger buffer
                relays: reply.relays.clone(),
                close_on_eose: true,
                cache_first: true,
                ..Default::default()
            });
        }

        parsed.root = self.get_thread_root(&event.tags);
        if let Some(ref root) = parsed.root {
            if root.id != event.id.to_hex() {
                requests.push(Request {
                    ids: vec![root.id.clone()],
                    limit: Some(3), // increase the limit to provide with a bigger buffer
                    relays: root.relays.clone(),
                    close_on_eose: true,
                    cache_first: true,
                    ..Default::default()
                });
            }
        }

        // Extract imeta tags from event for image metadata enrichment
        let imeta_map = extract_imeta_tags(&event.tags);

        // Extract emoji tags for NIP-30 custom emoji support
        let emoji_tags: Vec<Vec<String>> = event
            .tags
            .iter()
            .filter(|tag| tag.len() >= 3 && tag[0] == "emoji")
            .cloned()
            .collect();

        // Parse content into structured blocks with emoji support
        let content_parser = ContentParser::with_emojis(&emoji_tags);
        match content_parser.parse_content(&event.content) {
            Ok(mut content_blocks) => {
                // Enrich media with imeta data (images and videos)
                enrich_media_with_imeta(&mut content_blocks, &imeta_map);

                let parsed_blocks: Vec<ContentBlock> = content_blocks
                    .into_iter()
                    .map(|block| ContentBlock {
                        block_type: block.block_type,
                        text: block.text,
                        data: block.data,
                    })
                    .collect();

                // Create shortened content if needed
                let content_parser = ContentParser::new();
                let shortened_blocks =
                    content_parser.shorten_content(parsed_blocks.clone(), 500, 3, 10);

                parsed.parsed_content = parsed_blocks.clone();
                parsed.shortened_content =
                    if should_use_shortened_content(&parsed_blocks, &shortened_blocks) {
                        shortened_blocks
                    } else {
                        Vec::new()
                    };
            }
            Err(err) => {
                return Err(ParserError::Other(format!(
                    "error parsing content: {}",
                    err
                )));
            }
        }

        // Deduplicate requests using the utility
        let deduplicated_requests = RequestDeduplicator::deduplicate_requests(&requests);

        Ok((parsed, Some(deduplicated_requests)))
    }

    fn extract_profile_mentions(
        &self,
        content: &str,
        tags: &[Vec<String>],
        requests: &mut Vec<Request>,
    ) -> Vec<ProfilePointer> {
        let mut profile_mentions = Vec::new();

        // Look for nostr:npub... or npub... patterns
        for caps in PROFILE_RE.captures_iter(content) {
            if let Some(npub) = caps.get(1) {
                if let Ok(decoded) = nostr::nips::nip19::FromBech32::from_bech32(npub.as_str()) {
                    if let nostr::nips::nip19::Nip19::Pubkey(pubkey) = decoded {
                        let public_key = pubkey.to_string();
                        let relays = relay_hints_for_tag_value(tags, &["p"], &public_key);
                        let pointer = ProfilePointer { public_key, relays };

                        // Add request for this profile
                        requests.push(Request {
                            authors: vec![pointer.public_key.clone()],
                            kinds: vec![0],
                            limit: Some(1),
                            relays: pointer.relays.clone(),
                            close_on_eose: true,
                            cache_first: true,
                            ..Default::default()
                        });

                        profile_mentions.push(pointer);
                    }
                }
            }
        }

        // Also look for nprofile references
        for caps in NPROFILE_RE.captures_iter(content) {
            if let Some(nprofile) = caps.get(1) {
                if let Ok(decoded) = nostr::nips::nip19::FromBech32::from_bech32(nprofile.as_str())
                {
                    if let nostr::nips::nip19::Nip19::Profile(profile) = decoded {
                        let public_key = profile.public_key.to_string();
                        let relays = merge_relays(
                            profile
                                .relays
                                .into_iter()
                                .map(|url| url.to_string())
                                .collect(),
                            relay_hints_for_tag_value(tags, &["p"], &public_key),
                        );
                        let pointer = ProfilePointer { public_key, relays };

                        // Add request for this profile
                        requests.push(Request {
                            authors: vec![pointer.public_key.clone()],
                            kinds: vec![0],
                            limit: Some(1),
                            relays: pointer.relays.clone(),
                            close_on_eose: true,
                            cache_first: true,
                            ..Default::default()
                        });

                        profile_mentions.push(pointer);
                    }
                }
            }
        }

        profile_mentions
    }

    fn extract_event_refs(
        &self,
        content: &str,
        tags: &[Vec<String>],
        requests: &mut Vec<Request>,
    ) -> Vec<EventPointer> {
        let mut event_refs = Vec::new();

        // Look for nostr:note... or note... patterns
        for caps in NOTE_RE.captures_iter(content) {
            if let Some(note) = caps.get(1) {
                if let Ok(decoded) = nostr::nips::nip19::FromBech32::from_bech32(note.as_str()) {
                    if let nostr::nips::nip19::Nip19::EventId(event_id) = decoded {
                        let id = event_id.to_string();
                        let relays = relay_hints_for_tag_value(tags, &["e", "q"], &id);

                        // Add request for this event
                        requests.push(Request {
                            ids: vec![id.clone()],
                            limit: Some(3), // increase the limit to provide with a bigger buffer
                            relays: relays.clone(),
                            close_on_eose: true,
                            cache_first: true,
                            ..Default::default()
                        });

                        let pointer = EventPointer {
                            id,
                            relays,
                            author: None,
                            kind: None,
                        };
                        event_refs.push(pointer);
                    }
                }
            }
        }

        // Also look for nevent references
        for caps in NEVENT_RE.captures_iter(content) {
            if let Some(nevent) = caps.get(1) {
                if let Ok(decoded) = nostr::nips::nip19::FromBech32::from_bech32(nevent.as_str()) {
                    if let nostr::nips::nip19::Nip19::Event(event) = decoded {
                        let id = event.event_id.to_string();
                        let author = event.author.map(|pk| pk.to_string());
                        let relays = merge_relays(
                            event
                                .relays
                                .into_iter()
                                .map(|url| url.to_string())
                                .collect(),
                            relay_hints_for_tag_value(tags, &["e", "q"], &id),
                        );

                        // Add request for this event
                        requests.push(Request {
                            ids: vec![id.clone()],
                            limit: Some(3), // increase the limit to provide with a bigger buffer
                            relays: relays.clone(),
                            close_on_eose: true,
                            cache_first: true,
                            ..Default::default()
                        });

                        let pointer = EventPointer {
                            id,
                            relays,
                            author,
                            kind: None,
                        };
                        event_refs.push(pointer);
                    }
                }
            }
        }

        event_refs
    }

    fn extract_address_refs(
        &self,
        content: &str,
        requests: &mut Vec<Request>,
    ) -> Vec<AddressPointer> {
        let mut address_refs = Vec::new();

        for caps in NADDR_RE.captures_iter(content) {
            if let Some(naddr) = caps.get(1) {
                if let Ok(decoded) = nostr::nips::nip19::FromBech32::from_bech32(naddr.as_str()) {
                    if let nostr::nips::nip19::Nip19::Coordinate(coord) = decoded {
                        let pubkey = coord.public_key.to_string();
                        let d = coord.identifier;
                        let relays: Vec<String> = coord
                            .relays
                            .into_iter()
                            .map(|url| url.to_string())
                            .collect();

                        let pointer = AddressPointer {
                            kind: coord.kind as u64,
                            pubkey: pubkey.clone(),
                            d: d.clone(),
                            relays: relays.clone(),
                        };

                        let mut tags = FxHashMap::default();
                        tags.insert("#d".to_string(), vec![d]);
                        requests.push(Request {
                            authors: vec![pubkey],
                            kinds: vec![coord.kind as i32],
                            tags,
                            limit: Some(1),
                            relays,
                            close_on_eose: true,
                            cache_first: true,
                            ..Default::default()
                        });

                        address_refs.push(pointer);
                    }
                }
            }
        }

        address_refs
    }
}

// NEW: Build the FlatBuffer for Kind1Parsed
pub fn build_flatbuffer<'a, A: flatbuffers::Allocator + 'a>(
    parsed: &Kind1Parsed,
    builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
) -> Result<flatbuffers::WIPOffset<fb::Kind1Parsed<'a>>> {
    // Build content blocks vectors
    let mut parsed_content_offsets = Vec::new();
    for block in &parsed.parsed_content {
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
        parsed_content_offsets.push(content_block_offset);
    }
    let parsed_content_vector = builder.create_vector(&parsed_content_offsets);

    let mut shortened_content_offsets = Vec::new();
    for block in &parsed.shortened_content {
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
        shortened_content_offsets.push(content_block_offset);
    }
    let shortened_content_vector = if shortened_content_offsets.is_empty() {
        None
    } else {
        Some(builder.create_vector(&shortened_content_offsets))
    };

    // Build profile mentions (ProfilePointer)
    let mut profile_mentions_offsets = Vec::new();
    for mention in &parsed.profile_mentions {
        let public_key = builder.create_string(&mention.public_key);
        let relays_offsets: Vec<_> = mention
            .relays
            .iter()
            .map(|r| builder.create_string(r))
            .collect();
        let relays = if relays_offsets.is_empty() {
            None
        } else {
            Some(builder.create_vector(&relays_offsets))
        };

        let profile_pointer_args = fb::ProfilePointerArgs {
            public_key: Some(public_key),
            relays,
        };
        let profile_pointer_offset = fb::ProfilePointer::create(builder, &profile_pointer_args);
        profile_mentions_offsets.push(profile_pointer_offset);
    }
    let profile_mentions_vector = if profile_mentions_offsets.is_empty() {
        None
    } else {
        Some(builder.create_vector(&profile_mentions_offsets))
    };

    // Build event refs (EventPointer)
    let mut event_refs_offsets = Vec::new();
    for event_ref in &parsed.event_refs {
        let id = builder.create_string(&event_ref.id);
        let relays_offsets: Vec<_> = event_ref
            .relays
            .iter()
            .map(|r| builder.create_string(r))
            .collect();
        let relays = if relays_offsets.is_empty() {
            None
        } else {
            Some(builder.create_vector(&relays_offsets))
        };
        let author = event_ref.author.as_ref().map(|a| builder.create_string(a));

        let event_pointer_args = fb::EventPointerArgs {
            id: Some(id),
            relays,
            author,
            kind: event_ref.kind.unwrap_or(0),
        };
        let event_pointer_offset = fb::EventPointer::create(builder, &event_pointer_args);
        event_refs_offsets.push(event_pointer_offset);
    }
    let event_refs_vector = if event_refs_offsets.is_empty() {
        None
    } else {
        Some(builder.create_vector(&event_refs_offsets))
    };

    // Build address refs (AddressPointer)
    let mut address_refs_offsets = Vec::new();
    for address_ref in &parsed.address_refs {
        let pubkey = builder.create_string(&address_ref.pubkey);
        let d = builder.create_string(&address_ref.d);
        let relays_offsets: Vec<_> = address_ref
            .relays
            .iter()
            .map(|r| builder.create_string(r))
            .collect();
        let relays = if relays_offsets.is_empty() {
            None
        } else {
            Some(builder.create_vector(&relays_offsets))
        };

        let address_pointer_args = fb::AddressPointerArgs {
            kind: address_ref.kind,
            pubkey: Some(pubkey),
            d: Some(d),
            relays,
        };
        let address_pointer_offset = fb::AddressPointer::create(builder, &address_pointer_args);
        address_refs_offsets.push(address_pointer_offset);
    }
    let address_refs_vector = if address_refs_offsets.is_empty() {
        None
    } else {
        Some(builder.create_vector(&address_refs_offsets))
    };

    // Build reply EventPointer
    let reply = parsed.reply.as_ref().map(|r| {
        let id = builder.create_string(&r.id);
        let relays_offsets: Vec<_> = r
            .relays
            .iter()
            .map(|rel| builder.create_string(rel))
            .collect();
        let relays = if relays_offsets.is_empty() {
            None
        } else {
            Some(builder.create_vector(&relays_offsets))
        };
        let author = r.author.as_ref().map(|a| builder.create_string(a));

        let event_pointer_args = fb::EventPointerArgs {
            id: Some(id),
            relays,
            author,
            kind: r.kind.unwrap_or(0),
        };
        fb::EventPointer::create(builder, &event_pointer_args)
    });

    // Build root EventPointer
    let root = parsed.root.as_ref().map(|r| {
        let id = builder.create_string(&r.id);
        let relays_offsets: Vec<_> = r
            .relays
            .iter()
            .map(|rel| builder.create_string(rel))
            .collect();
        let relays = if relays_offsets.is_empty() {
            None
        } else {
            Some(builder.create_vector(&relays_offsets))
        };
        let author = r.author.as_ref().map(|a| builder.create_string(a));

        let event_pointer_args = fb::EventPointerArgs {
            id: Some(id),
            relays,
            author,
            kind: r.kind.unwrap_or(0),
        };
        fb::EventPointer::create(builder, &event_pointer_args)
    });

    let args = fb::Kind1ParsedArgs {
        parsed_content: Some(parsed_content_vector),
        shortened_content: shortened_content_vector,
        profile_mentions: profile_mentions_vector,
        event_refs: event_refs_vector,
        address_refs: address_refs_vector,
        reply,
        root,
    };

    let offset = fb::Kind1Parsed::create(builder, &args);

    Ok(offset)
}

fn is_hex64(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit()) && s == s.to_lowercase()
}

fn looks_like_marker(s: &str) -> bool {
    matches!(s, "reply" | "root" | "mention")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_use_shortened_content_when_text_changes_but_len_is_same() {
        let parsed = vec![ContentBlock::new(
            "text".to_string(),
            "line1\nline2".to_string(),
        )];
        let shortened = vec![ContentBlock::new(
            "text".to_string(),
            "line1...".to_string(),
        )];

        assert!(should_use_shortened_content(&parsed, &shortened));
    }

    #[test]
    fn test_should_not_use_shortened_content_when_identical() {
        let parsed = vec![ContentBlock::new("text".to_string(), "hello".to_string())];
        let shortened = vec![ContentBlock::new("text".to_string(), "hello".to_string())];

        assert!(!should_use_shortened_content(&parsed, &shortened));
    }

    #[test]
    fn test_should_not_use_shortened_content_when_empty() {
        let parsed = vec![ContentBlock::new("text".to_string(), "hello".to_string())];
        let shortened: Vec<ContentBlock> = Vec::new();

        assert!(!should_use_shortened_content(&parsed, &shortened));
    }

    #[test]
    fn test_relay_hints_for_tag_value_extracts_matching_p_tag_relays() {
        let pubkey = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let tags = vec![
            vec![
                "p".to_string(),
                pubkey.to_string(),
                "wss://relay.example".to_string(),
            ],
            vec![
                "p".to_string(),
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
                "wss://other.example".to_string(),
            ],
        ];

        assert_eq!(
            relay_hints_for_tag_value(&tags, &["p"], pubkey),
            vec!["wss://relay.example".to_string()]
        );
    }

    #[test]
    fn test_relay_hints_for_tag_value_extracts_matching_e_and_q_tag_relays() {
        let event_id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let tags = vec![
            vec![
                "e".to_string(),
                event_id.to_string(),
                "wss://e.example".to_string(),
            ],
            vec![
                "q".to_string(),
                event_id.to_string(),
                "wss://q.example".to_string(),
            ],
            vec!["q".to_string(), event_id.to_string(), String::new()],
        ];

        assert_eq!(
            relay_hints_for_tag_value(&tags, &["e", "q"], event_id),
            vec!["wss://e.example".to_string(), "wss://q.example".to_string()]
        );
    }

    #[test]
    fn test_merge_relays_keeps_nip19_relays_and_adds_tag_hints() {
        assert_eq!(
            merge_relays(
                vec![
                    "wss://nprofile.example".to_string(),
                    "wss://same.example".to_string()
                ],
                vec![
                    "wss://same.example".to_string(),
                    "wss://tag.example".to_string()
                ],
            ),
            vec![
                "wss://nprofile.example".to_string(),
                "wss://same.example".to_string(),
                "wss://tag.example".to_string(),
            ]
        );
    }
}

// Synchronous author guess based on the chosen e tag index.
// Strategy:
// 1) If e[3] looks like a hex pubkey and not a marker, use it (NIP-01 optional field).
// 2) Else map e-rank -> p-rank (counting only e/p tags).
// 3) If e-rank is 0 (root) and no p at same rank, use the first p.
// No current_event_pubkey filtering.
fn resolve_author_sync(tags: &[Vec<String>], chosen_e_abs_index: usize) -> Option<String> {
    if chosen_e_abs_index >= tags.len() {
        return None;
    }

    // Chosen e tag
    let e_tag = &tags[chosen_e_abs_index];

    // Step 1: NIP-01 optional author in e[3] (if not used by NIP-10 markers)
    if e_tag.len() >= 4 {
        let candidate = &e_tag[3];
        if is_hex64(candidate) && !looks_like_marker(candidate) {
            return Some(candidate.clone());
        }
    }

    // Step 2: compute e-rank (0-based) among only 'e' tags
    let mut e_rank = 0usize;
    for (i, tag) in tags.iter().enumerate() {
        if i == chosen_e_abs_index {
            break;
        }
        if tag.len() >= 2 && tag[0] == "e" {
            e_rank += 1;
        }
    }

    // Collect only 'p' tags
    let p_tags: Vec<&Vec<String>> = tags
        .iter()
        .filter(|t| t.len() >= 2 && t[0] == "p")
        .collect();

    // Step 3: map e-rank -> p-rank
    if e_rank < p_tags.len() && p_tags[e_rank].len() >= 2 {
        return Some(p_tags[e_rank][1].clone());
    }

    // Step 4: if chosen e is the root (rank 0) and no same-rank p, use first p
    if e_rank == 0 && !p_tags.is_empty() && p_tags[0].len() >= 2 {
        return Some(p_tags[0][1].clone());
    }

    None
}

impl Parser {
    fn get_immediate_parent(&self, tags: &[Vec<String>]) -> Option<EventPointer> {
        let mut reply_idx: Option<usize> = None;
        let mut last_e_idx: Option<usize> = None;

        for (i, tag) in tags.iter().enumerate() {
            if tag.len() >= 2 && tag[0] == "e" {
                last_e_idx = Some(i);
                if tag.len() >= 4 && tag[3] == "reply" {
                    reply_idx = Some(i);
                }
            }
        }

        let chosen_idx = reply_idx.or(last_e_idx)?;
        let e_tag = &tags[chosen_idx];

        if e_tag.len() >= 2 {
            let mut ptr = EventPointer {
                id: e_tag[1].clone(),
                relays: if e_tag.len() >= 3 && !e_tag[2].is_empty() {
                    vec![e_tag[2].clone()]
                } else {
                    Vec::new()
                },
                author: None,
                kind: None,
            };

            if let Some(author) = resolve_author_sync(tags, chosen_idx) {
                ptr.author = Some(author);
            }

            Some(ptr)
        } else {
            None
        }
    }

    fn get_thread_root(&self, tags: &[Vec<String>]) -> Option<EventPointer> {
        let mut root_idx: Option<usize> = None;
        let mut first_e_idx: Option<usize> = None;

        for (i, tag) in tags.iter().enumerate() {
            if tag.len() >= 2 && tag[0] == "e" {
                if first_e_idx.is_none() {
                    first_e_idx = Some(i);
                }
                if tag.len() >= 4 && tag[3] == "root" {
                    root_idx = Some(i);
                    break;
                }
            }
        }

        let chosen_idx = root_idx.or(first_e_idx)?;
        let e_tag = &tags[chosen_idx];

        if e_tag.len() >= 2 {
            let mut ptr = EventPointer {
                id: e_tag[1].clone(),
                relays: if e_tag.len() >= 3 && !e_tag[2].is_empty() {
                    vec![e_tag[2].clone()]
                } else {
                    Vec::new()
                },
                author: None,
                kind: None,
            };

            if let Some(author) = resolve_author_sync(tags, chosen_idx) {
                ptr.author = Some(author);
            }

            Some(ptr)
        } else {
            None
        }
    }
}

/// Extract imeta tags from event tags and build a map of URL -> ImetaData
/// imeta tags follow NIP-92 format: ["imeta", "url <url>", "dim <wxh>", "alt <text>", ...]
fn extract_imeta_tags(tags: &[Vec<String>]) -> std::collections::HashMap<String, ImetaData> {
    let mut imeta_map = std::collections::HashMap::new();

    for tag in tags {
        if tag.len() < 2 || tag[0] != "imeta" {
            continue;
        }

        let mut url = None;
        let mut alt = None;
        let mut dim = None;
        let mut blurhash = None;
        let mut mime_type = None;

        // Parse each field in the imeta tag (skip tag[0] which is "imeta")
        for field in &tag[1..] {
            if let Some((key, value)) = field.split_once(' ') {
                let value = value.trim();
                match key {
                    "url" => url = Some(value.to_string()),
                    "alt" => alt = Some(value.to_string()),
                    "dim" => dim = Some(value.to_string()),
                    "blurhash" => blurhash = Some(value.to_string()),
                    "m" => mime_type = Some(value.to_string()),
                    _ => {} // Ignore unknown fields
                }
            }
        }

        // Only add if we have a URL
        if let Some(url) = url {
            imeta_map.insert(
                url.clone(),
                ImetaData {
                    url,
                    alt,
                    dim,
                    blurhash,
                    mime_type,
                },
            );
        }
    }

    imeta_map
}
