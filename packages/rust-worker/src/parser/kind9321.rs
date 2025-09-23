use crate::nostr::Template;
use crate::parser::Parser;
use crate::parser::{ParserError, Result};
use crate::signer::interface::SignerManagerInterface;
use crate::types::network::Request;
use crate::types::nostr::Event;
use crate::types::proof::Proof;
use crate::utils::request_deduplication::RequestDeduplicator;
use rustc_hash::FxHashMap;
use tracing::{debug, error, info, warn};

// NEW: Imports for FlatBuffers
use crate::generated::nostr::*;

pub struct Kind9321Parsed {
    pub amount: i32,
    pub recipient: String,
    pub event_id: Option<String>,
    pub mint_url: String,
    pub redeemed: bool,
    pub proofs: Vec<Proof>,
    pub comment: Option<String>,
    pub is_p2pk_locked: bool,
    pub p2pk_pubkey: Option<String>,
}

impl Parser {
    pub fn parse_kind_9321(&self, event: &Event) -> Result<(Kind9321Parsed, Option<Vec<Request>>)> {
        if event.kind != 9321 {
            return Err(ParserError::Other("event is not kind 9321".to_string()));
        }

        let mut requests = Vec::new();

        // Get the sender profile for this nutzap
        requests.push(Request {
            authors: vec![event.pubkey.to_hex()],
            kinds: vec![0],
            relays: self
                .database
                .find_relay_candidates(0, &event.pubkey.to_hex(), &false),
            close_on_eose: true,
            cache_first: true,
            ..Default::default()
        });

        // Extract required tags
        let mut proof_tags = Vec::new();
        let mut mint_tag: Option<Vec<String>> = None;
        let mut recipient_tag: Option<Vec<String>> = None;
        let mut event_tag: Option<Vec<String>> = None;

        for tag in &event.tags {
            if tag.len() >= 2 {
                match tag[0].as_str() {
                    "proof" => proof_tags.push(tag.to_vec()),
                    "u" => {
                        if mint_tag.is_none() {
                            mint_tag = Some(tag.to_vec());
                        }
                    }
                    "p" => {
                        if recipient_tag.is_none() {
                            recipient_tag = Some(tag.to_vec());
                            // Get recipient profile
                            requests.push(Request {
                                authors: vec![tag[1].clone()],
                                kinds: vec![0],
                                relays: self.database.find_relay_candidates(0, &tag[1], &false),
                                cache_first: true,
                                ..Default::default()
                            });

                            // Check for spending history events
                            let mut spending_tags = FxHashMap::default();
                            spending_tags.insert("#e".to_string(), vec![event.id.to_hex()]);

                            requests.push(Request {
                                authors: vec![tag[1].clone()],
                                kinds: vec![7376],
                                tags: spending_tags,
                                limit: Some(1),
                                relays: self.database.find_relay_candidates(7376, &tag[1], &false),
                                cache_first: true,
                                ..Default::default()
                            });
                        }
                    }
                    "e" => {
                        if event_tag.is_none() {
                            event_tag = Some(tag.to_vec());
                            requests.push(Request {
                                ids: vec![tag[1].clone()],
                                kinds: vec![1],
                                relays: self.database.find_relay_candidates(1, "", &false),
                                cache_first: true,
                                ..Default::default()
                            });
                        }
                    }
                    _ => {}
                }
            }
        }

        // Validate essential tags are present
        if proof_tags.is_empty() || mint_tag.is_none() || recipient_tag.is_none() {
            return Err(ParserError::Other("missing required tags".to_string()));
        }

        let mint_url = mint_tag.as_ref().unwrap()[1].clone();
        let recipient = recipient_tag.as_ref().unwrap()[1].clone();

        // Parse nutzap information
        let mut total = 0i32;
        let mut proofs = Vec::new();
        let mut is_p2pk_locked = false;
        let mut p2pk_pubkey: Option<String> = None;

        for proof_tag in proof_tags {
            match Proof::from_json(&proof_tag[1]) {
                Ok(proof) => {
                    total += proof.amount as i32;

                    // Check for P2PK locking in the secret field
                    if proof.secret.contains("P2PK") {
                        is_p2pk_locked = true;
                        if let Some(pubkey) = parse_p2pk_pubkey(&proof.secret) {
                            if p2pk_pubkey.is_none() {
                                p2pk_pubkey = Some(pubkey);
                            }
                        }
                    }
                    proofs.push(proof);
                }
                Err(e) => {
                    warn!(
                        "Failed to parse proof from tag: {} - Error: {}",
                        proof_tag[1], e
                    );
                }
            }
        }

        let result = Kind9321Parsed {
            amount: total,
            recipient,
            mint_url,
            proofs,
            redeemed: false, // Default to not redeemed, will check later
            comment: if event.content.is_empty() {
                None
            } else {
                Some(event.content.clone())
            },
            is_p2pk_locked,
            p2pk_pubkey,
            event_id: event_tag.map(|tag| tag[1].clone()),
        };

        // Deduplicate requests using the utility
        let deduplicated_requests = RequestDeduplicator::deduplicate_requests(&requests);

        Ok((result, Some(deduplicated_requests)))
    }

    pub fn prepare_kind_9321(&self, template: &Template) -> Result<Event> {
        if template.kind != 9321 {
            return Err(ParserError::Other("event is not kind 9321".to_string()));
        }

        // Validate required tags
        let mut has_proof = false;
        let mut has_mint = false;
        let mut has_recipient = false;

        for tag in &template.tags {
            if tag.len() >= 2 {
                match tag[0].as_str() {
                    "proof" => has_proof = true,
                    "u" => has_mint = true,
                    "p" => has_recipient = true,
                    _ => {}
                }
            }
        }

        if !has_proof {
            return Err(ParserError::Other(
                "kind 9321 must include at least one proof tag".to_string(),
            ));
        }

        if !has_mint {
            return Err(ParserError::Other(
                "kind 9321 must include a u tag with mint URL".to_string(),
            ));
        }

        if !has_recipient {
            return Err(ParserError::Other(
                "kind 9321 must include a p tag with recipient".to_string(),
            ));
        }

        self.signer_manager
            .sign_event(template)
            .map_err(|e| ParserError::Other(format!("failed to sign event: {}", e)))
    }
}

fn parse_p2pk_pubkey(secret: &str) -> Option<String> {
    let bytes = secret.as_bytes();
    if bytes.first()? != &b'[' {
        return None;
    }

    let mut pos = 1; // skip '['

    // Skip whitespace
    while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
        pos += 1;
    }

    // Parse first string "P2PK"
    if pos >= bytes.len() || bytes[pos] != b'"' {
        return None;
    }
    pos += 1; // skip '"'
    let start = pos;
    while pos < bytes.len() && bytes[pos] != b'"' {
        if bytes[pos] == b'\\' {
            pos += 1; // skip escape
        }
        pos += 1;
    }
    if pos >= bytes.len() || std::str::from_utf8(&bytes[start..pos]).ok()? != "P2PK" {
        return None;
    }
    pos += 1; // skip closing '"'

    // Skip whitespace and comma
    while pos < bytes.len() && (bytes[pos].is_ascii_whitespace() || bytes[pos] == b',') {
        pos += 1;
    }

    // Parse second element as object
    if pos >= bytes.len() || bytes[pos] != b'{' {
        return None;
    }

    // Find "data" field
    let remaining = std::str::from_utf8(&bytes[pos..]).ok()?;
    if let Some(data_start) = remaining.find(r#""data""#) {
        let data_pos = pos + data_start + 7; // "data": (7 chars including quotes and colon)
                                             // Skip whitespace after colon
        let mut data_pos = data_pos;
        while data_pos < bytes.len() && bytes[data_pos].is_ascii_whitespace() {
            data_pos += 1;
        }
        if data_pos < bytes.len() && bytes[data_pos] == b'"' {
            data_pos += 1; // skip opening '"'
            let value_start = data_pos;
            while data_pos < bytes.len() && bytes[data_pos] != b'"' {
                if bytes[data_pos] == b'\\' {
                    data_pos += 1; // skip escape
                }
                data_pos += 1;
            }
            if data_pos < bytes.len() {
                return std::str::from_utf8(&bytes[value_start..data_pos])
                    .ok()
                    .map(|s| s.to_string());
            }
        }
    }

    None
}

// NEW: Build the FlatBuffer for Kind9321Parsed
pub fn build_flatbuffer<'a, A: flatbuffers::Allocator + 'a>(
    parsed: &Kind9321Parsed,
    builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
) -> Result<flatbuffers::WIPOffset<fb::Kind9321Parsed<'a>>> {
    let recipient = builder.create_string(&parsed.recipient);
    let event_id = parsed.event_id.as_ref().map(|id| builder.create_string(id));
    let mint_url = builder.create_string(&parsed.mint_url);
    let comment = parsed.comment.as_ref().map(|c| builder.create_string(c));
    let p2pk_pubkey = parsed
        .p2pk_pubkey
        .as_ref()
        .map(|p| builder.create_string(p));

    // Build proofs vector
    let mut proofs_offsets = Vec::new();
    for proof in &parsed.proofs {
        proofs_offsets.push(proof.to_offset(builder));
    }
    let proofs_vector = builder.create_vector(&proofs_offsets);

    if parsed.recipient.is_empty() {
        error!("Kind9321 recipient is empty!");
    }
    if parsed.mint_url.is_empty() {
        error!("Kind9321 mint_url is empty!");
    }

    let args = fb::Kind9321ParsedArgs {
        amount: parsed.amount,
        recipient: Some(recipient),
        event_id,
        mint_url: Some(mint_url),
        redeemed: parsed.redeemed,
        proofs: Some(proofs_vector),
        comment,
        is_p2pk_locked: parsed.is_p2pk_locked,
        p2pk_pubkey,
    };

    let offset = fb::Kind9321Parsed::create(builder, &args);

    Ok(offset)
}
