use crate::parser::{
    content::{parse_content, ContentBlock, ContentParser},
    Parser,
};
use crate::types::network::Request;
use crate::utils::request_deduplication::RequestDeduplicator;
use anyhow::{anyhow, Result};
use nostr::{Event, Tag};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfilePointer {
    pub public_key: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub relays: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventPointer {
    pub id: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub relays: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Kind1Parsed {
    #[serde(rename = "parsedContent", default)]
    pub parsed_content: Vec<ContentBlock>,
    #[serde(
        rename = "shortenedContent",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub shortened_content: Vec<ContentBlock>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub quotes: Vec<ProfilePointer>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub mentions: Vec<EventPointer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply: Option<EventPointer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root: Option<EventPointer>,
}

impl Parser {
    pub fn parse_kind_1(&self, event: &Event) -> Result<(Kind1Parsed, Option<Vec<Request>>)> {
        if event.kind.as_u64() != 1 {
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
                if let Ok(decoded) = nostr::nips::nip19::FromBech32::from_bech32(npub.as_str()) {
                    if let nostr::nips::nip19::Nip19::Pubkey(pubkey) = decoded {
                        let pointer = ProfilePointer {
                            public_key: pubkey.to_string(),
                            relays: Vec::new(),
                        };
                        quotes.push(pointer.clone());

                        // Add request for this profile
                        requests.push(Request {
                            authors: vec![pointer.public_key],
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
                    }
                }
            }
        }

        // Also look for nprofile references
        let nprofile_regex = Regex::new(r"(?:nostr:)?(nprofile1[a-z0-9]+)").unwrap();

        for caps in nprofile_regex.captures_iter(content) {
            if let Some(nprofile) = caps.get(1) {
                if let Ok(decoded) = nostr::nips::nip19::FromBech32::from_bech32(nprofile.as_str())
                {
                    if let nostr::nips::nip19::Nip19::Profile(profile) = decoded {
                        let pointer = ProfilePointer {
                            public_key: profile.public_key.to_string(),
                            relays: profile
                                .relays
                                .into_iter()
                                .map(|url| url.to_string())
                                .collect(),
                        };
                        quotes.push(pointer.clone());

                        // Add request for this profile
                        requests.push(Request {
                            authors: vec![pointer.public_key],
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
                if let Ok(decoded) = nostr::nips::nip19::FromBech32::from_bech32(note.as_str()) {
                    if let nostr::nips::nip19::Nip19::EventId(event_id) = decoded {
                        let pointer = EventPointer {
                            id: event_id.to_string(),
                            relays: Vec::new(),
                            author: None,
                            kind: None,
                        };
                        mentions.push(pointer.clone());

                        // Add request for this event
                        requests.push(Request {
                            ids: vec![pointer.id],
                            limit: Some(3), // increase the limit to provide with a bigger buffer
                            relays: self.database.find_relay_candidates(1, "", &false),
                            close_on_eose: true,
                            cache_first: true,
                            ..Default::default()
                        });
                    }
                }
            }
        }

        // Also look for nevent references
        let nevent_regex = Regex::new(r"(?:nostr:)?(nevent1[a-z0-9]+)").unwrap();

        for caps in nevent_regex.captures_iter(content) {
            if let Some(nevent) = caps.get(1) {
                if let Ok(decoded) = nostr::nips::nip19::FromBech32::from_bech32(nevent.as_str()) {
                    if let nostr::nips::nip19::Nip19::Event(event) = decoded {
                        let pointer = EventPointer {
                            id: event.event_id.to_string(),
                            relays: event
                                .relays
                                .into_iter()
                                .map(|url| url.to_string())
                                .collect(),
                            author: event.author.map(|pk| pk.to_string()),
                            kind: None,
                        };
                        mentions.push(pointer.clone());

                        // Add request for this event
                        requests.push(Request {
                            ids: vec![pointer.id],
                            limit: Some(3), // increase the limit to provide with a bigger buffer
                            relays: self.database.find_relay_candidates(
                                1,
                                &pointer.author.as_deref().unwrap_or(""),
                                &false,
                            ),
                            close_on_eose: true,
                            cache_first: true,
                            ..Default::default()
                        });
                    }
                }
            }
        }

        mentions
    }

    fn get_immediate_parent(&self, tags: &[Tag]) -> Option<EventPointer> {
        // Find the last 'e' tag with 'reply' marker or the last 'e' tag if no markers
        let mut reply_tag = None;
        let mut last_e_tag = None;

        for tag in tags {
            let tag_vec = tag.as_vec();
            if tag_vec.len() >= 2 && tag_vec[0] == "e" {
                last_e_tag = Some(tag);

                // Check if this has a 'reply' marker
                if tag_vec.len() >= 4 && tag_vec[3] == "reply" {
                    reply_tag = Some(tag);
                }
            }
        }

        let chosen_tag = reply_tag.or(last_e_tag)?;
        let tag_vec = chosen_tag.as_vec();

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

    fn get_thread_root(&self, tags: &[Tag]) -> Option<EventPointer> {
        // Find the first 'e' tag with 'root' marker or the first 'e' tag if no markers
        let mut root_tag = None;
        let mut first_e_tag = None;

        for tag in tags {
            let tag_vec = tag.as_vec();
            if tag_vec.len() >= 2 && tag_vec[0] == "e" {
                if first_e_tag.is_none() {
                    first_e_tag = Some(tag);
                }

                // Check if this has a 'root' marker
                if tag_vec.len() >= 4 && tag_vec[3] == "root" {
                    root_tag = Some(tag);
                    break; // Found explicit root, use it
                }
            }
        }

        let chosen_tag = root_tag.or(first_e_tag)?;
        let tag_vec = chosen_tag.as_vec();

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::request_deduplication::RequestDeduplicator;
    use nostr::{EventBuilder, Keys, Kind, Tag};

    #[test]
    fn test_parse_kind_1_basic() {
        let keys = Keys::generate();
        let content = "Hello, Nostr world!";

        let event = EventBuilder::new(Kind::TextNote, content, Vec::new())
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, requests) = parser.parse_kind_1(&event).unwrap();

        assert_eq!(parsed.parsed_content.len(), 1);
        assert_eq!(parsed.parsed_content[0].block_type, "text");
        assert_eq!(parsed.parsed_content[0].text, content);
        assert_eq!(parsed.shortened_content.len(), 0); // Content is short, no shortening needed
        assert!(requests.is_some());
        assert!(requests.unwrap().len() >= 2); // Author profile + relay list
    }

    #[test]
    fn test_parse_kind_1_with_reply() {
        let keys = Keys::generate();
        let content = "This is a reply";
        let reply_event_id = "1234567890abcdef1234567890abcdef12345678";
        let relay_url = "wss://relay.example.com";

        let tags = vec![Tag::parse(vec![
            "e".to_string(),
            reply_event_id.to_string(),
            relay_url.to_string(),
            "reply".to_string(),
        ])
        .unwrap()];

        let event = EventBuilder::new(Kind::TextNote, content, tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, _) = parser.parse_kind_1(&event).unwrap();

        assert!(parsed.reply.is_some());
        let reply = parsed.reply.unwrap();
        assert_eq!(reply.id, reply_event_id);
        assert_eq!(reply.relays, vec![relay_url]);
        assert_eq!(parsed.shortened_content.len(), 0); // Content is short, no shortening needed
    }

    #[test]
    fn test_parse_kind_1_with_root() {
        let keys = Keys::generate();
        let content = "This is part of a thread";
        let root_event_id = "abcdef1234567890abcdef1234567890abcdef12";
        let relay_url = "wss://relay.example.com";

        let tags = vec![Tag::parse(vec![
            "e".to_string(),
            root_event_id.to_string(),
            relay_url.to_string(),
            "root".to_string(),
        ])
        .unwrap()];

        let event = EventBuilder::new(Kind::TextNote, content, tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, _) = parser.parse_kind_1(&event).unwrap();

        assert!(parsed.root.is_some());
        let root = parsed.root.unwrap();
        assert_eq!(root.id, root_event_id);
        assert_eq!(root.relays, vec![relay_url]);
        assert_eq!(parsed.shortened_content.len(), 0); // Content is short, no shortening needed
    }

    #[test]
    fn test_parse_wrong_kind() {
        let keys = Keys::generate();

        let event = EventBuilder::new(Kind::Metadata, "{}", Vec::new())
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let result = parser.parse_kind_1(&event);

        assert!(result.is_err());
    }

    #[test]
    fn test_parse_kind_1_with_hashtags() {
        let keys = Keys::generate();
        let content = "I love #bitcoin and #nostr!";

        let event = EventBuilder::new(Kind::TextNote, content, Vec::new())
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, _) = parser.parse_kind_1(&event).unwrap();

        // Should have parsed hashtags in content
        assert!(parsed.parsed_content.len() > 1);
        let has_hashtag = parsed
            .parsed_content
            .iter()
            .any(|block| block.block_type == "hashtag");
        assert!(has_hashtag);
        assert_eq!(parsed.shortened_content.len(), 0); // Content is short, no shortening needed
    }

    #[test]
    fn test_request_deduplication() {
        // Create multiple requests with the same filter criteria but different relays
        let requests = vec![
            Request {
                ids: vec!["event1".to_string(), "event2".to_string()],
                authors: vec!["author1".to_string()],
                kinds: vec![1, 6],
                relays: vec!["relay1.com".to_string(), "relay2.com".to_string()],
                limit: Some(1),
                close_on_eose: true,
                cache_first: true,
                ..Default::default()
            },
            Request {
                ids: vec!["event2".to_string(), "event1".to_string()], // Same IDs, different order
                authors: vec!["author1".to_string()],
                kinds: vec![6, 1], // Same kinds, different order
                relays: vec!["relay2.com".to_string(), "relay3.com".to_string()],
                limit: Some(1),
                close_on_eose: true,
                cache_first: true,
                ..Default::default()
            },
            Request {
                ids: vec!["event3".to_string()],
                authors: vec!["author2".to_string()],
                kinds: vec![1],
                relays: vec!["relay1.com".to_string()],
                limit: Some(1),
                close_on_eose: true,
                cache_first: true,
                ..Default::default()
            },
            Request {
                ids: vec!["event1".to_string(), "event2".to_string()],
                authors: vec!["author1".to_string()],
                kinds: vec![1, 6],
                relays: vec!["relay4.com".to_string(), "relay1.com".to_string()],
                limit: Some(2), // Different limit - should NOT be deduplicated
                close_on_eose: true,
                cache_first: true,
                ..Default::default()
            },
        ];

        let deduplicated = RequestDeduplicator::deduplicate_requests(requests);

        // Should have 3 unique requests (2 with same filter but different limits, 1 unique)
        assert_eq!(deduplicated.len(), 3);

        // Find requests with event1 and event2
        let matching_requests: Vec<_> = deduplicated
            .iter()
            .filter(|r| {
                r.ids.contains(&"event1".to_string()) && r.ids.contains(&"event2".to_string())
            })
            .collect();

        // Should have 2 requests with event1+event2 (different limits)
        assert_eq!(matching_requests.len(), 2);

        // Find the request with limit 1
        let limit_1_request = matching_requests
            .iter()
            .find(|r| r.limit == Some(1))
            .unwrap();

        // Should have 3 relays deduplicated
        assert_eq!(limit_1_request.relays.len(), 3);
        assert!(limit_1_request.relays.contains(&"relay1.com".to_string()));
        assert!(limit_1_request.relays.contains(&"relay2.com".to_string()));
        assert!(limit_1_request.relays.contains(&"relay3.com".to_string()));

        // Find the request with limit 2
        let limit_2_request = matching_requests
            .iter()
            .find(|r| r.limit == Some(2))
            .unwrap();

        // Should have 2 relays
        assert_eq!(limit_2_request.relays.len(), 2);
        assert!(limit_2_request.relays.contains(&"relay1.com".to_string()));
        assert!(limit_2_request.relays.contains(&"relay4.com".to_string()));

        // Find the request with event3
        let single_event_request = deduplicated
            .iter()
            .find(|r| r.ids.contains(&"event3".to_string()))
            .unwrap();

        // Should have only one relay
        assert_eq!(single_event_request.relays.len(), 1);
        assert!(single_event_request
            .relays
            .contains(&"relay1.com".to_string()));
    }

    #[test]
    fn test_parse_kind_1_with_long_content() {
        let keys = Keys::generate();
        // Create content longer than 500 characters
        let long_content =
            "This is a very long text that should be shortened when parsed. ".repeat(20); // ~1280 characters

        let event = EventBuilder::new(Kind::TextNote, &long_content, Vec::new())
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, _) = parser.parse_kind_1(&event).unwrap();

        // Should have parsed content
        assert_eq!(parsed.parsed_content.len(), 1);
        assert_eq!(parsed.parsed_content[0].block_type, "text");
        assert_eq!(parsed.parsed_content[0].text, long_content);

        // Should have shortened content since it's longer than 500 characters
        assert!(parsed.shortened_content.len() > 0);
        assert_eq!(parsed.shortened_content.len(), 1);
        assert_eq!(parsed.shortened_content[0].block_type, "text");
        assert!(parsed.shortened_content[0].text.len() < long_content.len());
        assert!(parsed.shortened_content[0].text.ends_with("..."));
    }

    #[test]
    fn test_parse_kind_1_with_many_line_breaks() {
        let keys = Keys::generate();
        // Create content with many line breaks (15 lines, but short total length)
        let content_with_lines = "Line 1\nLine 2\nLine 3\nLine 4\nLine 5\nLine 6\nLine 7\nLine 8\nLine 9\nLine 10\nLine 11\nLine 12\nLine 13\nLine 14\nLine 15";

        let event = EventBuilder::new(Kind::TextNote, content_with_lines, Vec::new())
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, _) = parser.parse_kind_1(&event).unwrap();

        // Should have parsed content
        assert_eq!(parsed.parsed_content.len(), 1);
        assert_eq!(parsed.parsed_content[0].block_type, "text");
        assert_eq!(parsed.parsed_content[0].text, content_with_lines);

        // Should have shortened content since it has more than 10 lines
        assert!(parsed.shortened_content.len() > 0);
        assert_eq!(parsed.shortened_content.len(), 1);
        assert_eq!(parsed.shortened_content[0].block_type, "text");
        assert!(parsed.shortened_content[0].text.lines().count() <= 10);
        assert!(parsed.shortened_content[0].text.ends_with("..."));
    }

    #[test]
    fn test_parse_kind_1_with_long_text_and_many_lines() {
        let keys = Keys::generate();
        // Create content with both long lines and many lines (should trigger both limits)
        let long_line =
            "This is a very long line that exceeds typical length limits and should be truncated. "
                .repeat(10); // ~870 chars per line
        let content_with_long_lines = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
            long_line,
            long_line,
            long_line,
            long_line,
            long_line,
            long_line,
            long_line,
            long_line,
            long_line,
            long_line,
            long_line,
            long_line
        ); // 12 lines, each ~870 chars

        let event = EventBuilder::new(Kind::TextNote, &content_with_long_lines, Vec::new())
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, _) = parser.parse_kind_1(&event).unwrap();

        // Should have parsed content
        assert_eq!(parsed.parsed_content.len(), 1);
        assert_eq!(parsed.parsed_content[0].block_type, "text");
        assert_eq!(parsed.parsed_content[0].text, content_with_long_lines);

        // Should have shortened content due to both length and line count limits
        assert!(parsed.shortened_content.len() > 0);
        assert_eq!(parsed.shortened_content.len(), 1);
        assert_eq!(parsed.shortened_content[0].block_type, "text");
        assert!(parsed.shortened_content[0].text.len() < content_with_long_lines.len());
        assert!(parsed.shortened_content[0].text.lines().count() <= 10);
        assert!(parsed.shortened_content[0].text.ends_with("..."));
    }
}
