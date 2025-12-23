use crate::parser::{Parser, ParserError, Result};

use shared::{
    generated::nostr::*,
    types::{
        network::Request,
        nostr::{NostrTags, Template},
        Event,
    },
};

pub struct HistoryTag {
    pub name: String,
    pub value: String,
    pub relay: Option<String>,
    pub marker: Option<String>,
}

pub struct Kind7376Parsed {
    pub direction: String, // "in" or "out"
    pub amount: i32,       // Amount in sats
    pub created_events: Vec<String>,
    pub destroyed_events: Vec<String>,
    pub redeemed_events: Vec<String>,
    pub tags: Vec<HistoryTag>,
    pub decrypted: bool,
}

impl Parser {
    pub async fn parse_kind_7376(
        &self,
        event: &Event,
    ) -> Result<(Kind7376Parsed, Option<Vec<Request>>)> {
        if event.kind != 7376 {
            return Err(ParserError::Other("event is not kind 7376".to_string()));
        }

        let mut requests = Vec::new();
        let mut parsed = Kind7376Parsed {
            direction: String::new(),
            amount: 0,
            created_events: Vec::new(),
            destroyed_events: Vec::new(),
            redeemed_events: Vec::new(),
            tags: Vec::new(),
            decrypted: false,
        };

        // Process unencrypted e tags with "redeemed" marker
        for tag in &event.tags {
            if tag.len() >= 4 && tag[0] == "e" && tag[3] == "redeemed" {
                parsed.redeemed_events.push(tag[1].clone());
                // Add request for this event
                requests.push(Request {
                    ids: vec![tag[1].clone()],
                    kinds: vec![7375],
                    relays: vec![],
                    cache_first: true,
                    ..Default::default()
                });
            }
        }

        // Attempt to decrypt spending history (NIP-44) using the sender's pubkey
        let sender_pubkey = event.pubkey.to_string();
        if let Ok(decrypted) = self
            .crypto_client
            .nip44_decrypt(&sender_pubkey, &event.content)
            .await
        {
            if !decrypted.is_empty() {
                if let Ok(tags) = NostrTags::from_json(&decrypted) {
                    parsed.decrypted = true;
                    parsed.tags = Vec::new();

                    // Process decrypted tags - access inner Vec<Vec<String>> via .0
                    for tag in &tags.0 {
                        if tag.len() >= 2 {
                            let history_tag = HistoryTag {
                                name: tag[0].clone(),
                                value: tag[1].clone(),
                                relay: tag.get(2).cloned(),
                                marker: tag.get(3).cloned(),
                            };
                            parsed.tags.push(history_tag);

                            // Extract specific tag values
                            match tag[0].as_str() {
                                "direction" => parsed.direction = tag[1].clone(),
                                "amount" => {
                                    if let Ok(amt) = tag[1].parse::<i32>() {
                                        parsed.amount = amt;
                                    }
                                }
                                "e" => {
                                    if tag.len() >= 4 {
                                        match tag[3].as_str() {
                                            "created" => parsed.created_events.push(tag[1].clone()),
                                            "destroyed" => {
                                                parsed.destroyed_events.push(tag[1].clone())
                                            }
                                            "redeemed" => {
                                                parsed.redeemed_events.push(tag[1].clone())
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }

        Ok((parsed, Some(requests)))
    }

    pub async fn prepare_kind_7376(&self, template: &Template) -> Result<Event> {
        if template.kind != 7376 {
            return Err(ParserError::Other("event is not kind 7376".to_string()));
        }

        let tags: NostrTags = NostrTags::from_json(&template.content)
            .map_err(|e| ParserError::Other(format!("invalid spending history content: {}", e)))?;

        // Check for required direction and amount tags - access inner Vec via .0
        let mut has_direction = false;
        let mut has_amount = false;

        for tag in &tags.0 {
            if tag.len() >= 2 {
                match tag[0].as_str() {
                    "direction" => {
                        has_direction = true;
                        if tag[1] != "in" && tag[1] != "out" {
                            return Err(ParserError::Other(
                                "direction must be 'in' or 'out'".to_string(),
                            ));
                        }
                    }
                    "amount" => has_amount = true,
                    _ => {}
                }
            }
        }

        if !has_direction || !has_amount {
            return Err(ParserError::Other(
                "spending history must include direction and amount".to_string(),
            ));
        }

        let tags_json = tags.to_json();

        // Using signer's own pubkey by passing empty recipient to nip44_encrypt

        let encrypted = self
            .crypto_client
            .nip44_encrypt("", &tags_json)
            .await
            .map_err(|e| ParserError::Crypto(format!("Signer error: {}", e)))?;

        let encrypted_template = Template::new(template.kind, encrypted, template.tags.clone());

        // Sign the event
        let signed_event_json = self
            .crypto_client
            .sign_event(encrypted_template.to_json())
            .await
            .map_err(|e| ParserError::Crypto(format!("Signer error: {}", e)))?;

        let new_event = Event::from_json(&signed_event_json)?;
        Ok(new_event)
    }
}

// NEW: Build the FlatBuffer for Kind7376Parsed
pub fn build_flatbuffer<'a, A: flatbuffers::Allocator + 'a>(
    parsed: &Kind7376Parsed,
    builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
) -> Result<flatbuffers::WIPOffset<fb::Kind7376Parsed<'a>>> {
    let direction = builder.create_string(&parsed.direction);

    // Build created_events vector
    let created_events_offsets: Vec<_> = parsed
        .created_events
        .iter()
        .map(|id| builder.create_string(id))
        .collect();
    let created_events_vector = if created_events_offsets.is_empty() {
        None
    } else {
        Some(builder.create_vector(&created_events_offsets))
    };

    // Build destroyed_events vector
    let destroyed_events_offsets: Vec<_> = parsed
        .destroyed_events
        .iter()
        .map(|id| builder.create_string(id))
        .collect();
    let destroyed_events_vector = if destroyed_events_offsets.is_empty() {
        None
    } else {
        Some(builder.create_vector(&destroyed_events_offsets))
    };

    // Build redeemed_events vector
    let redeemed_events_offsets: Vec<_> = parsed
        .redeemed_events
        .iter()
        .map(|id| builder.create_string(id))
        .collect();
    let redeemed_events_vector = if redeemed_events_offsets.is_empty() {
        None
    } else {
        Some(builder.create_vector(&redeemed_events_offsets))
    };

    // Build tags vector
    let mut tags_offsets = Vec::new();
    for tag in &parsed.tags {
        let name = builder.create_string(&tag.name);
        let value = builder.create_string(&tag.value);
        let relay = tag.relay.as_ref().map(|r| builder.create_string(r));
        let marker = tag.marker.as_ref().map(|m| builder.create_string(m));

        let history_tag_args = fb::HistoryTagArgs {
            name: Some(name),
            value: Some(value),
            relay,
            marker,
        };
        let history_tag_offset = fb::HistoryTag::create(builder, &history_tag_args);
        tags_offsets.push(history_tag_offset);
    }
    let tags_vector = if tags_offsets.is_empty() {
        None
    } else {
        Some(builder.create_vector(&tags_offsets))
    };

    let args = fb::Kind7376ParsedArgs {
        direction: Some(direction),
        amount: parsed.amount,
        created_events: created_events_vector,
        destroyed_events: destroyed_events_vector,
        redeemed_events: redeemed_events_vector,
        tags: tags_vector,
        decrypted: parsed.decrypted,
    };

    let offset = fb::Kind7376Parsed::create(builder, &args);

    Ok(offset)
}
