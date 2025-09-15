use crate::parser::{
    content::{parse_content, serialize_content_data, ContentBlock, ContentParser},
    Parser,
};
use crate::types::network::Request;
use crate::types::nostr::Event;
use crate::utils::request_deduplication::RequestDeduplicator;
use anyhow::{anyhow, Result};

// NEW: Imports for FlatBuffers
use crate::generated::nostr::*;

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

pub struct Kind1Parsed {
    pub parsed_content: Vec<ContentBlock>,
    pub shortened_content: Vec<ContentBlock>,
    pub quotes: Vec<ProfilePointer>,
    pub mentions: Vec<EventPointer>,
    pub reply: Option<EventPointer>,
    pub root: Option<EventPointer>,
}

impl Parser {
    pub fn parse_kind_1(&self, event: &Event) -> Result<(Kind1Parsed, Option<Vec<Request>>)> {
        if event.kind != 1 {
            return Err(anyhow!("event is not kind 1"));
        }

        let mut requests = Vec::new();
        let mut parsed = Kind1Parsed {
            parsed_content: Vec::new(),
            shortened_content: Vec::new(),
            quotes: Vec::new(),
            mentions: Vec::new(),
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
        parsed.quotes = self.extract_profile_mentions(&event.content, &mut requests);
        parsed.mentions = self.extract_event_mentions(&event.content, &mut requests);

        // Extract reply and root using NIP-10
        parsed.reply = self.get_immediate_parent(&event.tags);
        if let Some(ref reply) = parsed.reply {
            requests.push(Request {
                ids: vec![reply.id.clone()],
                limit: Some(3), // increase the limit to provide with a bigger buffer
                relays: {
                    let mut combined_relays = reply.relays.clone();
                    combined_relays.extend(self.database.find_relay_candidates(
                        1,
                        reply.author.as_deref().unwrap_or(""),
                        &true,
                    ));
                    combined_relays
                },
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
                    relays: {
                        let mut combined_relays = root.relays.clone();
                        combined_relays.extend(self.database.find_relay_candidates(
                            1,
                            root.author.as_deref().unwrap_or(""),
                            &true,
                        ));
                        combined_relays
                    },
                    close_on_eose: true,
                    cache_first: true,
                    ..Default::default()
                });
            }
        }

        // Parse content into structured blocks
        match parse_content(&event.content) {
            Ok(content_blocks) => {
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
                parsed.shortened_content = if shortened_blocks.len() < parsed_blocks.len() {
                    shortened_blocks
                } else {
                    Vec::new()
                };
            }
            Err(err) => {
                return Err(anyhow!("error parsing content: {}", err));
            }
        }

        // Deduplicate requests using the utility
        let deduplicated_requests = RequestDeduplicator::deduplicate_requests(requests);

        Ok((parsed, Some(deduplicated_requests)))
    }

    fn extract_profile_mentions(
        &self,
        content: &str,
        requests: &mut Vec<Request>,
    ) -> Vec<ProfilePointer> {
        use regex::Regex;
        let mut quotes = Vec::new();

        // Look for nostr:npub... or npub... patterns
        let profile_regex = Regex::new(r"(?:nostr:)?(npub1[a-z0-9]+)").unwrap();

        for caps in profile_regex.captures_iter(content) {
            if let Some(npub) = caps.get(1) {
                if let Ok(decoded) =
                    crate::types::nostr::nips::nip19::FromBech32::from_bech32(npub.as_str())
                {
                    if let crate::types::nostr::nips::nip19::Nip19::Pubkey(pubkey) = decoded {
                        let pointer = ProfilePointer {
                            public_key: pubkey.to_string(),
                            relays: Vec::new(),
                        };

                        // Add request for this profile
                        requests.push(Request {
                            authors: vec![pointer.public_key.clone()],
                            kinds: vec![0],
                            limit: Some(1),
                            relays: self.database.find_relay_candidates(
                                0,
                                &pubkey.to_string(),
                                &false,
                            ),
                            close_on_eose: true,
                            cache_first: true,
                            ..Default::default()
                        });

                        quotes.push(pointer);
                    }
                }
            }
        }

        // Also look for nprofile references
        let nprofile_regex = Regex::new(r"(?:nostr:)?(nprofile1[a-z0-9]+)").unwrap();

        for caps in nprofile_regex.captures_iter(content) {
            if let Some(nprofile) = caps.get(1) {
                if let Ok(decoded) =
                    crate::types::nostr::nips::nip19::FromBech32::from_bech32(nprofile.as_str())
                {
                    if let crate::types::nostr::nips::nip19::Nip19::Profile(profile) = decoded {
                        let pointer = ProfilePointer {
                            public_key: profile.public_key.to_string(),
                            relays: profile
                                .relays
                                .into_iter()
                                .map(|url| url.to_string())
                                .collect(),
                        };

                        // Add request for this profile
                        requests.push(Request {
                            authors: vec![pointer.public_key.clone()],
                            kinds: vec![0],
                            limit: Some(1),
                            relays: self.database.find_relay_candidates(
                                0,
                                &profile.public_key.to_string(),
                                &false,
                            ),
                            close_on_eose: true,
                            cache_first: true,
                            ..Default::default()
                        });

                        quotes.push(pointer);
                    }
                }
            }
        }

        quotes
    }

    fn extract_event_mentions(
        &self,
        content: &str,
        requests: &mut Vec<Request>,
    ) -> Vec<EventPointer> {
        use regex::Regex;
        let mut mentions = Vec::new();

        // Look for nostr:note... or note... patterns
        let note_regex = Regex::new(r"(?:nostr:)?(note1[a-z0-9]+)").unwrap();

        for caps in note_regex.captures_iter(content) {
            if let Some(note) = caps.get(1) {
                if let Ok(decoded) =
                    crate::types::nostr::nips::nip19::FromBech32::from_bech32(note.as_str())
                {
                    if let crate::types::nostr::nips::nip19::Nip19::EventId(event_id) = decoded {
                        let id = event_id.to_string();

                        // Add request for this event
                        requests.push(Request {
                            ids: vec![id.clone()],
                            limit: Some(3), // increase the limit to provide with a bigger buffer
                            relays: self.database.find_relay_candidates(1, "", &false),
                            close_on_eose: true,
                            cache_first: true,
                            ..Default::default()
                        });

                        let pointer = EventPointer {
                            id,
                            relays: Vec::new(),
                            author: None,
                            kind: None,
                        };
                        mentions.push(pointer);
                    }
                }
            }
        }

        // Also look for nevent references
        let nevent_regex = Regex::new(r"(?:nostr:)?(nevent1[a-z0-9]+)").unwrap();

        for caps in nevent_regex.captures_iter(content) {
            if let Some(nevent) = caps.get(1) {
                if let Ok(decoded) =
                    crate::types::nostr::nips::nip19::FromBech32::from_bech32(nevent.as_str())
                {
                    if let crate::types::nostr::nips::nip19::Nip19::Event(event) = decoded {
                        let id = event.event_id.to_string();
                        let author = event.author.map(|pk| pk.to_string());

                        // Add request for this event
                        requests.push(Request {
                            ids: vec![id.clone()],
                            limit: Some(3), // increase the limit to provide with a bigger buffer
                            relays: self.database.find_relay_candidates(
                                1,
                                &author.as_deref().unwrap_or(""),
                                &false,
                            ),
                            close_on_eose: true,
                            cache_first: true,
                            ..Default::default()
                        });

                        let pointer = EventPointer {
                            id,
                            relays: event
                                .relays
                                .into_iter()
                                .map(|url| url.to_string())
                                .collect(),
                            author,
                            kind: None,
                        };
                        mentions.push(pointer);
                    }
                }
            }
        }

        mentions
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

    // Build quotes (ProfilePointer)
    let mut quotes_offsets = Vec::new();
    for quote in &parsed.quotes {
        let public_key = builder.create_string(&quote.public_key);
        let relays_offsets: Vec<_> = quote
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
        quotes_offsets.push(profile_pointer_offset);
    }
    let quotes_vector = if quotes_offsets.is_empty() {
        None
    } else {
        Some(builder.create_vector(&quotes_offsets))
    };

    // Build mentions (EventPointer)
    let mut mentions_offsets = Vec::new();
    for mention in &parsed.mentions {
        let id = builder.create_string(&mention.id);
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
        let author = mention.author.as_ref().map(|a| builder.create_string(a));

        let event_pointer_args = fb::EventPointerArgs {
            id: Some(id),
            relays,
            author,
            kind: mention.kind.unwrap_or(0),
        };
        let event_pointer_offset = fb::EventPointer::create(builder, &event_pointer_args);
        mentions_offsets.push(event_pointer_offset);
    }
    let mentions_vector = if mentions_offsets.is_empty() {
        None
    } else {
        Some(builder.create_vector(&mentions_offsets))
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
        quotes: quotes_vector,
        mentions: mentions_vector,
        reply,
        root,
    };

    let offset = fb::Kind1Parsed::create(builder, &args);

    Ok(offset)
}

impl Parser {
    fn get_immediate_parent(&self, tags: &[Vec<String>]) -> Option<EventPointer> {
        // Find the last 'e' tag with 'reply' marker or the last 'e' tag if no markers
        let mut reply_tag = None;
        let mut last_e_tag = None;

        for tag in tags {
            if tag.len() >= 2 && tag[0] == "e" {
                last_e_tag = Some(tag);

                // Check if this has a 'reply' marker
                if tag.len() >= 4 && tag[3] == "reply" {
                    reply_tag = Some(tag);
                }
            }
        }

        let chosen_tag = reply_tag.or(last_e_tag)?;
        let tag_vec = chosen_tag;

        if tag_vec.len() >= 2 {
            Some(EventPointer {
                id: tag_vec[1].clone(),
                relays: if tag_vec.len() >= 3 && !tag_vec[2].is_empty() {
                    vec![tag_vec[2].clone()]
                } else {
                    Vec::new()
                },
                author: None,
                kind: None,
            })
        } else {
            None
        }
    }

    fn get_thread_root(&self, tags: &[Vec<String>]) -> Option<EventPointer> {
        // Find the first 'e' tag with 'root' marker or the first 'e' tag if no markers
        let mut root_tag = None;
        let mut first_e_tag = None;

        for tag in tags {
            if tag.len() >= 2 && tag[0] == "e" {
                if first_e_tag.is_none() {
                    first_e_tag = Some(tag);
                }

                // Check if this has a 'root' marker
                if tag.len() >= 4 && tag[3] == "root" {
                    root_tag = Some(tag);
                    break; // Found explicit root, use it
                }
            }
        }

        let tag_vec = root_tag.or(first_e_tag)?;

        if tag_vec.len() >= 2 {
            Some(EventPointer {
                id: tag_vec[1].clone(),
                relays: if tag_vec.len() >= 3 && !tag_vec[2].is_empty() {
                    vec![tag_vec[2].clone()]
                } else {
                    Vec::new()
                },
                author: None,
                kind: None,
            })
        } else {
            None
        }
    }
}
