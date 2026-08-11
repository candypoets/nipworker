use crate::generated::nostr::*;
use crate::parser::{Parser, ParserError, Result};
use crate::types::network::Request;
use crate::types::Event;
use rustc_hash::FxHashMap;

/// Definition kinds a badge award may reference. NIP-58 badges use kind
/// 30009; NIP-97 extends awards to entitlement definitions backed by NIP-99
/// listings (30402) and calendar events (31922/31923).
pub const BADGE_DEFINITION_KINDS: [u64; 4] = [30009, 30402, 31922, 31923];

#[derive(Debug, Clone)]
pub struct BadgeAwardRecipient {
    pub pubkey: String,
    pub relay: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Kind8Parsed {
    pub badge_address: String,
    pub badge_relay: Option<String>,
    pub recipients: Vec<BadgeAwardRecipient>,
    pub content: String,
}

/// Parse a definition address of the form "<kind>:<pubkey>:<d>".
fn parse_definition_address(address: &str) -> Option<(u64, String, String)> {
    let mut parts = address.splitn(3, ':');
    let kind = parts.next()?.parse::<u64>().ok()?;
    let author = parts.next()?;
    let d = parts.next()?;
    if author.len() != 64 || !author.chars().all(|c| c.is_ascii_hexdigit()) || d.is_empty() {
        return None;
    }
    Some((kind, author.to_string(), d.to_string()))
}

impl Parser {
    pub fn parse_kind_8(&self, event: &Event) -> Result<(Kind8Parsed, Option<Vec<Request>>)> {
        if event.kind != 8 {
            return Err(ParserError::Other("event is not kind 8".to_string()));
        }

        let badge_tag = event
            .tags
            .iter()
            .find(|tag| tag.len() >= 2 && tag[0] == "a")
            .ok_or_else(|| ParserError::MissingField("badge address tag".to_string()))?;

        let badge_address = badge_tag[1].clone();
        let badge_relay = badge_tag.get(2).cloned().filter(|relay| !relay.is_empty());

        let (definition_kind, definition_author, definition_d) =
            parse_definition_address(&badge_address).ok_or_else(|| {
                ParserError::InvalidTag(
                    "badge award a tag must be a <kind>:<pubkey>:<d> address".to_string(),
                )
            })?;

        if !BADGE_DEFINITION_KINDS.contains(&definition_kind) {
            return Err(ParserError::InvalidTag(format!(
                "badge award a tag must reference a NIP-97 definition kind (30009, 30402, 31922, 31923), got kind {definition_kind}"
            )));
        }

        let recipients: Vec<_> = event
            .tags
            .iter()
            .filter(|tag| tag.len() >= 2 && tag[0] == "p")
            .map(|tag| BadgeAwardRecipient {
                pubkey: tag[1].clone(),
                relay: tag.get(2).cloned().filter(|relay| !relay.is_empty()),
            })
            .collect();

        if recipients.is_empty() {
            return Err(ParserError::MissingField(
                "badge award recipient p tag".to_string(),
            ));
        }

        // An award is only meaningful together with its definition, so follow
        // up by fetching it, honoring the optional relay hint on the a tag.
        let mut tags = FxHashMap::default();
        tags.insert("#d".to_string(), vec![definition_d]);
        let requests = vec![Request {
            authors: vec![definition_author],
            kinds: vec![definition_kind as i32],
            tags,
            relays: badge_relay.clone().into_iter().collect(),
            close_on_eose: true,
            cache_first: true,
            ..Default::default()
        }];

        Ok((
            Kind8Parsed {
                badge_address,
                badge_relay,
                recipients,
                content: event.content.clone(),
            },
            Some(requests),
        ))
    }
}

pub fn build_flatbuffer<'a, A: flatbuffers::Allocator + 'a>(
    parsed: &Kind8Parsed,
    builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
) -> Result<flatbuffers::WIPOffset<fb::Kind8Parsed<'a>>> {
    let badge_address = builder.create_string(&parsed.badge_address);
    let badge_relay = parsed
        .badge_relay
        .as_ref()
        .map(|relay| builder.create_string(relay));
    let content = if parsed.content.is_empty() {
        None
    } else {
        Some(builder.create_string(&parsed.content))
    };

    let recipient_offsets: Vec<_> = parsed
        .recipients
        .iter()
        .map(|recipient| {
            let pubkey = builder.create_string(&recipient.pubkey);
            let relay = recipient
                .relay
                .as_ref()
                .map(|relay| builder.create_string(relay));

            fb::BadgeAwardRecipient::create(
                builder,
                &fb::BadgeAwardRecipientArgs {
                    pubkey: Some(pubkey),
                    relay,
                },
            )
        })
        .collect();
    let recipients = builder.create_vector(&recipient_offsets);

    Ok(fb::Kind8Parsed::create(
        builder,
        &fb::Kind8ParsedArgs {
            badge_address: Some(badge_address),
            badge_relay,
            recipients: Some(recipients),
            content,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::Parser;

    const EVENT_ID: &str = "0000000000000000000000000000000000000000000000000000000000000001";
    const ISSUER: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const MEMBER: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const SIG: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

    fn award_event(tags: Vec<Vec<String>>) -> Event {
        let tags_json = tags
            .iter()
            .map(|tag| {
                format!(
                    "[{}]",
                    tag.iter()
                        .map(|value| format!("\"{value}\""))
                        .collect::<Vec<_>>()
                        .join(",")
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let json = format!(
            r#"{{"id":"{EVENT_ID}","pubkey":"{ISSUER}","created_at":1700000000,"kind":8,"tags":[{tags_json}],"content":"","sig":"{SIG}"}}"#
        );
        Event::from_json(&json).expect("valid award event json")
    }

    fn parse(tags: Vec<Vec<String>>) -> Result<(Kind8Parsed, Option<Vec<Request>>)> {
        Parser::new(None).parse_kind_8(&award_event(tags))
    }

    fn award_tags(address: &str) -> Vec<Vec<String>> {
        vec![
            vec!["a".to_string(), address.to_string()],
            vec!["p".to_string(), MEMBER.to_string()],
        ]
    }

    #[test]
    fn nip58_badge_award_requests_30009_definition() {
        let address = format!("30009:{ISSUER}:members");
        let (parsed, requests) = parse(award_tags(&address)).expect("30009 award parses");

        assert_eq!(parsed.badge_address, address);
        assert_eq!(parsed.recipients.len(), 1);
        assert_eq!(parsed.recipients[0].pubkey, MEMBER);

        let requests = requests.expect("definition fetch request emitted");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].kinds, vec![30009]);
        assert_eq!(requests[0].authors, vec![ISSUER.to_string()]);
        assert_eq!(
            requests[0].tags.get("#d"),
            Some(&vec!["members".to_string()])
        );
        assert!(requests[0].close_on_eose);
        assert!(requests[0].cache_first);
    }

    #[test]
    fn nip97_listing_award_is_accepted() {
        let address = format!("30402:{ISSUER}:three-visit-pass");
        let (parsed, requests) = parse(award_tags(&address)).expect("30402 award parses");

        assert_eq!(parsed.badge_address, address);
        let requests = requests.expect("definition fetch request emitted");
        assert_eq!(requests[0].kinds, vec![30402]);
        assert_eq!(
            requests[0].tags.get("#d"),
            Some(&vec!["three-visit-pass".to_string()])
        );
    }

    #[test]
    fn nip97_calendar_event_awards_are_accepted() {
        for kind in [31922, 31923] {
            let address = format!("{kind}:{ISSUER}:gig-12-31");
            parse(award_tags(&address)).unwrap_or_else(|e| panic!("{address} parses: {e}"));
        }
    }

    #[test]
    fn award_rejects_unknown_definition_kind() {
        let address = format!("30023:{ISSUER}:blog-post");
        let result = parse(award_tags(&address));
        assert!(matches!(result, Err(ParserError::InvalidTag(_))));
    }

    #[test]
    fn award_rejects_malformed_address() {
        for address in [
            "members".to_string(),
            format!("30009:{ISSUER}"),
            format!("30009:{ISSUER}:"),
            format!("30009:not-a-pubkey:members"),
        ] {
            let result = parse(award_tags(&address));
            assert!(
                matches!(result, Err(ParserError::InvalidTag(_))),
                "{address} must be rejected"
            );
        }
    }

    #[test]
    fn award_requires_badge_address_and_recipient() {
        let no_a_tag = parse(vec![vec!["p".to_string(), MEMBER.to_string()]]);
        assert!(matches!(no_a_tag, Err(ParserError::MissingField(_))));

        let address = format!("30009:{ISSUER}:members");
        let no_p_tag = parse(vec![vec!["a".to_string(), address]]);
        assert!(matches!(no_p_tag, Err(ParserError::MissingField(_))));
    }

    #[test]
    fn award_definition_request_honors_relay_hint() {
        let address = format!("30009:{ISSUER}:members");
        let tags = vec![
            vec![
                "a".to_string(),
                address,
                "wss://hints.example.com".to_string(),
            ],
            vec!["p".to_string(), MEMBER.to_string()],
        ];
        let (parsed, requests) = parse(tags).expect("award with relay hint parses");

        assert_eq!(
            parsed.badge_relay,
            Some("wss://hints.example.com".to_string())
        );
        let requests = requests.expect("definition fetch request emitted");
        assert_eq!(
            requests[0].relays,
            vec!["wss://hints.example.com".to_string()]
        );
    }
}
