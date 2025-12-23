use crate::parser::Parser;
use crate::parser::{ParserError, Result};

use crate::utils::json::BaseJsonParser;
use crate::utils::request_deduplication::RequestDeduplicator;
use std::fmt::Write;

// NEW: Imports for FlatBuffers
use shared::generated::nostr::*;
use shared::types::network::Request;
use shared::types::nostr::{NostrTags, Template};
use shared::types::Event;

/// ZapRequest for Nostr kind 9735 zap receipts
#[derive(Debug, Clone)]
pub struct ZapRequest {
    pub kind: u16,
    pub pubkey: String,
    pub content: String,
    pub tags: NostrTags,
    pub signature: Option<String>,
}

impl ZapRequest {
    /// Parse ZapRequest from JSON string
    pub fn from_json(json: &str) -> Result<Self> {
        let parser = ZapRequestParser::new(json.as_bytes());
        parser.parse()
    }

    /// Serialize ZapRequest to JSON string
    pub fn to_json(&self) -> String {
        let mut result = String::with_capacity(self.calculate_json_size());

        result.push('{');

        // Required fields
        write!(
            result,
            r#""kind":{},"pubkey":"{}","content":"{}","tags":{}"#,
            self.kind,
            Self::escape_string(&self.pubkey),
            Self::escape_string(&self.content),
            self.tags.to_json()
        )
        .unwrap();

        // Optional signature field
        if let Some(ref signature) = self.signature {
            result.push_str(r#","signature":""#);
            Self::escape_string_to(&mut result, signature);
            result.push('"');
        }

        result.push('}');
        result
    }

    #[inline(always)]
    fn calculate_json_size(&self) -> usize {
        let mut size = 50; // Base structure

        // Required fields
        size += self.pubkey.len() * 2 + self.content.len() * 2; // Escaping
        size += self.tags.calculate_json_size(); // Tags size

        // Optional signature
        if let Some(ref signature) = self.signature {
            size += 15 + signature.len() * 2; // "signature":"" + escaping
        }

        size
    }

    #[inline(always)]
    fn escape_string(s: &str) -> String {
        if !s.contains('\\') && !s.contains('"') {
            s.to_string()
        } else {
            let mut result = String::with_capacity(s.len() + 4);
            Self::escape_string_to(&mut result, s);
            result
        }
    }

    #[inline(always)]
    fn escape_string_to(result: &mut String, s: &str) {
        for ch in s.chars() {
            match ch {
                '\\' => result.push_str("\\\\"),
                '"' => result.push_str("\\\""),
                '\n' => result.push_str("\\n"),
                '\r' => result.push_str("\\r"),
                '\t' => result.push_str("\\t"),
                other => result.push(other),
            }
        }
    }
}

enum ZapRequestParserData<'a> {
    Borrowed(&'a [u8]),
    Owned(Vec<u8>),
}

struct ZapRequestParser<'a> {
    data: ZapRequestParserData<'a>,
}

impl<'a> ZapRequestParser<'a> {
    #[inline(always)]
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            data: ZapRequestParserData::Borrowed(bytes),
        }
    }

    #[inline(always)]
    fn parse(mut self) -> Result<ZapRequest> {
        // Get the bytes to parse
        let bytes = match &self.data {
            ZapRequestParserData::Borrowed(b) => *b,
            ZapRequestParserData::Owned(v) => v.as_slice(),
        };

        // Handle escaped JSON if needed
        let mut parser = if let Some(unescaped) = BaseJsonParser::unescape_if_needed(bytes)? {
            // Use the unescaped data
            self.data = ZapRequestParserData::Owned(unescaped);
            match &self.data {
                ZapRequestParserData::Owned(v) => BaseJsonParser::new(v.as_slice()),
                _ => unreachable!(),
            }
        } else {
            BaseJsonParser::new(bytes)
        };

        parser.skip_whitespace();
        parser.expect_byte(b'{')?;

        let mut kind = 0u16;
        let mut pubkey = String::new();
        let mut content = String::new();
        let mut tags = NostrTags(Vec::new());
        let mut signature = None;

        while parser.pos < parser.bytes.len() {
            parser.skip_whitespace();
            if parser.peek() == b'}' {
                parser.pos += 1;
                break;
            }

            let key = parser.parse_string()?;
            parser.skip_whitespace();
            parser.expect_byte(b':')?;
            parser.skip_whitespace();

            match key {
                "kind" => kind = parser.parse_u64()? as u16,
                "pubkey" => pubkey = parser.parse_string_unescaped()?,
                "content" => content = parser.parse_string_unescaped()?,
                "tags" => tags = NostrTags::from_json(parser.parse_raw_json_value()?)?,
                "signature" => signature = Some(parser.parse_string_unescaped()?),
                _ => parser.skip_value()?,
            }

            parser.skip_comma_or_end()?;
        }

        if pubkey.is_empty() || content.is_empty() {
            return Err(ParserError::InvalidFormat(
                "Missing required fields in ZapRequest".to_string(),
            ));
        }

        Ok(ZapRequest {
            kind,
            pubkey,
            content,
            tags,
            signature,
        })
    }
}

pub struct Kind9735Parsed {
    pub id: String,
    pub amount: i32,
    pub content: String,
    pub bolt11: String,
    pub preimage: Option<String>,
    pub sender: String,
    pub recipient: String,
    pub event: Option<String>,
    pub event_coordinate: Option<String>,
    pub timestamp: u64,
    pub valid: bool,
    pub description: ZapRequest,
}

fn sats_from_bolt11_hrp(invoice: &str) -> Option<i64> {
    // Normalize to lowercase for HRP parsing
    let lower = invoice.to_ascii_lowercase();
    if !lower.starts_with("ln") {
        return None;
    }
    // Split HRP and data part
    let (hrp, _) = lower.split_once('1')?;
    if hrp.len() <= 2 {
        return None; // no currency nor amount
    }
    // Remove "ln"
    let hrp = &hrp[2..];

    // Find where amount digits start (after currency code, e.g., "bc", "tb", "bcrt")
    let start = hrp.find(|c: char| c.is_ascii_digit())?;
    let amount_part = &hrp[start..];
    if amount_part.is_empty() {
        return None;
    }

    // Split into numeric part and optional suffix unit
    let (digits_str, unit_opt) = {
        let last = amount_part.chars().last().unwrap();
        if last.is_ascii_alphabetic() {
            (&amount_part[..amount_part.len() - 1], Some(last))
        } else {
            (amount_part, None)
        }
    };

    if digits_str.is_empty() {
        return None;
    }

    // Use wide integers to avoid overflow during conversions
    let value = digits_str.parse::<u128>().ok()?;

    let msat: u128 = match unit_opt {
        None => value.saturating_mul(1000),             // sats
        Some('m') => value.saturating_mul(100_000_000), // milli-BTC
        Some('u') => value.saturating_mul(100_000),     // micro-BTC
        Some('n') => value.saturating_mul(100),         // nano-BTC
        Some('p') => {
            // pico-BTC = 0.1 msat per unit; must be multiple of 10 to land on whole msats
            if value % 10 != 0 {
                return None;
            }
            value / 10
        }
        _ => return None,
    };

    // Always return sats (integer). Sub-sat totals become 0 and are treated as None.
    let sats_u128 = msat / 1000;
    if sats_u128 == 0 {
        return None;
    }
    i64::try_from(sats_u128).ok()
}

impl Parser {
    pub fn parse_kind_9735(&self, event: &Event) -> Result<(Kind9735Parsed, Option<Vec<Request>>)> {
        if event.kind != 9735 {
            return Err(ParserError::Other("event is not kind 9735".to_string()));
        }

        let mut requests = Vec::new();

        // Get the sender profile for this zap
        requests.push(Request {
            authors: vec![event.pubkey.to_hex()],
            kinds: vec![0],
            relays: vec![],
            cache_first: true,
            ..Default::default()
        });

        // Extract tags
        let mut p_tag: Option<Vec<String>> = None;
        let mut e_tag: Option<Vec<String>> = None;
        let mut a_tag: Option<Vec<String>> = None;
        let mut bolt11_tag: Option<Vec<String>> = None;
        let mut description_tag: Option<Vec<String>> = None;
        let mut preimage_tag: Option<Vec<String>> = None;
        let mut sender_tag: Option<Vec<String>> = None;

        for tag in &event.tags {
            if tag.len() >= 2 {
                match tag[0].as_str() {
                    "p" => p_tag = Some(tag.to_vec()),
                    "e" => e_tag = Some(tag.to_vec()),
                    "a" => a_tag = Some(tag.to_vec()),
                    "bolt11" => bolt11_tag = Some(tag.to_vec()),
                    "description" => description_tag = Some(tag.to_vec()),
                    "preimage" => preimage_tag = Some(tag.to_vec()),
                    "P" => sender_tag = Some(tag.to_vec()), // Capital P for sender
                    _ => {}
                }
            }
        }

        // Require mandatory tags
        if p_tag.is_none() || bolt11_tag.is_none() || description_tag.is_none() {
            return Err(ParserError::Other("missing required tags".to_string()));
        }

        let recipient = p_tag.as_ref().unwrap()[1].clone();
        let bolt11 = bolt11_tag.as_ref().unwrap()[1].clone();
        let description_str = description_tag.as_ref().unwrap()[1].clone();

        // ✅ UPDATED: Parse the zap request using our custom parser
        let zap_request: ZapRequest = ZapRequest::from_json(&description_str).map_err(|e| {
            ParserError::Other(format!("failed to parse zap request description: {}", e))
        })?;

        // Validate that the zap request is properly formed
        if zap_request.kind != 9734 || zap_request.tags.0.is_empty() {
            return Err(ParserError::Other("invalid zap request".to_string()));
        }

        // Extract amount from bolt11 invoice or zap request
        let mut amount = 0i32;

        // First check if there's an amount tag in the zap request
        if let Some(amount_tag) = find_tag_in_vec(&zap_request.tags.0, "amount") {
            if amount_tag.len() >= 2 {
                if let Ok(amt_int) = amount_tag[1].parse::<i64>() {
                    amount = (amt_int / 1000) as i32; // Convert from millisats to sats
                }
            }
        }
        // Fallback: If no amount tag or 0 sats, decode BOLT11 HRP to get sats
        if amount == 0 {
            if let Some(sats) = sats_from_bolt11_hrp(&bolt11) {
                if let Ok(sats_i32) = i32::try_from(sats) {
                    amount = sats_i32; // always sats
                }
            }
        }

        // Determine sender
        let sender = if let Some(sender_tag) = sender_tag {
            sender_tag[1].clone()
        } else {
            zap_request.pubkey.clone()
        };

        // Extract relay hints from the zap request
        let mut zapper_relay_hints = Vec::new();
        if let Some(relays_tag) = find_tag_in_vec(&zap_request.tags.0, "relays") {
            if relays_tag.len() > 1 {
                zapper_relay_hints = relays_tag[1..].to_vec();
            }
        }

        // Try to find the zapper profile
        requests.push(Request {
            authors: vec![event.pubkey.to_hex()],
            kinds: vec![0],
            limit: Some(1),
            relays: zapper_relay_hints,
            cache_first: true,
            ..Default::default()
        });

        // Create the parsed zap receipt
        let mut receipt = Kind9735Parsed {
            id: event.id.to_hex(),
            amount,
            content: zap_request.content.clone(),
            bolt11,
            sender,
            recipient: recipient.clone(),
            timestamp: event.created_at,
            valid: true, // We'll validate below
            description: zap_request,
            preimage: preimage_tag.map(|tag| tag[1].clone()),
            event: e_tag.map(|tag| tag[1].clone()),
            event_coordinate: a_tag.map(|tag| tag[1].clone()),
        };

        // Perform basic validation
        // 1. The zap request should have the same recipient as the receipt
        if let Some(request_p_tag) = find_tag_in_vec(&receipt.description.tags.0, "p") {
            if request_p_tag.len() >= 2 && request_p_tag[1] != receipt.recipient {
                receipt.valid = false;
            }
        } else {
            receipt.valid = false;
        }

        // 2. If the receipt has an event ID, the request should also have it
        if let Some(ref event_id) = receipt.event {
            if let Some(request_e_tag) = find_tag_in_vec(&receipt.description.tags.0, "e") {
                if request_e_tag.len() < 2 || request_e_tag[1] != *event_id {
                    receipt.valid = false;
                }
            } else {
                receipt.valid = false;
            }
        }

        // 3. If the receipt has an event coordinate, the request should also have it
        if let Some(ref event_coordinate) = receipt.event_coordinate {
            if let Some(request_a_tag) = find_tag_in_vec(&receipt.description.tags.0, "a") {
                if request_a_tag.len() < 2 || request_a_tag[1] != *event_coordinate {
                    receipt.valid = false;
                }
            } else {
                receipt.valid = false;
            }
        }

        // Deduplicate requests using the utility
        let deduplicated_requests = RequestDeduplicator::deduplicate_requests(&requests);

        Ok((receipt, Some(deduplicated_requests)))
    }

    pub async fn prepare_kind_9735(&self, template: &Template) -> Result<Event> {
        if template.kind != 9735 {
            return Err(ParserError::Other("event is not kind 9735".to_string()));
        }

        // ✅ UPDATED: Validate zap request using our custom parser
        let zap_request: ZapRequest = ZapRequest::from_json(&template.content)
            .map_err(|e| ParserError::Other(format!("invalid zap request content: {}", e)))?;

        // Validate that the zap request is properly formed
        if zap_request.kind != 9734 || zap_request.tags.0.is_empty() {
            return Err(ParserError::Other("invalid zap request".to_string()));
        }

        // Check for required tags in the zap request
        let mut has_recipient = false;
        let mut has_amount = false;

        for tag in &zap_request.tags.0 {
            if tag.len() >= 2 {
                match tag[0].as_str() {
                    "p" => has_recipient = true,
                    "amount" => has_amount = true,
                    _ => {}
                }
            }
        }

        if !has_recipient {
            return Err(ParserError::Other(
                "zap request must include a recipient (p tag)".to_string(),
            ));
        }

        if !has_amount {
            return Err(ParserError::Other(
                "zap request must include an amount".to_string(),
            ));
        }

        // For zap receipts, the content should be empty (description goes in tags)
        // But we'll keep the existing logic for now
        let content = String::new();

        // Add description tag with the serialized zap request
        let mut tags = template.tags.clone();
        let description_json = zap_request.to_json();
        tags.push(vec!["description".to_string(), description_json]);

        let new_template = Template {
            kind: template.kind,
            tags,
            content,
            created_at: template.created_at,
        };

        let signed_event_json = self
            .crypto_client
            .sign_event(new_template.to_json())
            .await
            .map_err(|e| ParserError::Crypto(format!("Signer error: {}", e)))?;

        let new_event = Event::from_json(&signed_event_json)?;
        Ok(new_event)
    }
}

// NEW: Build the FlatBuffer for Kind9735Parsed
pub fn build_flatbuffer<'a, A: flatbuffers::Allocator + 'a>(
    parsed: &Kind9735Parsed,
    builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
) -> Result<flatbuffers::WIPOffset<fb::Kind9735Parsed<'a>>> {
    let id = builder.create_string(&parsed.id);
    let content = builder.create_string(&parsed.content);
    let bolt11 = builder.create_string(&parsed.bolt11);
    let preimage = parsed.preimage.as_ref().map(|p| builder.create_string(p));
    let sender = builder.create_string(&parsed.sender);
    let recipient = builder.create_string(&parsed.recipient);
    let event = parsed.event.as_ref().map(|e| builder.create_string(e));
    let event_coordinate = parsed
        .event_coordinate
        .as_ref()
        .map(|ec| builder.create_string(ec));

    // Build ZapRequest
    let description_content = builder.create_string(&parsed.description.content);
    let description_pubkey = builder.create_string(&parsed.description.pubkey);
    let description_signature = parsed
        .description
        .signature
        .as_ref()
        .map(|s| builder.create_string(s));

    // Build description tags - access inner Vec via .0
    let mut description_tags_offsets = Vec::new();
    for tag in &parsed.description.tags.0 {
        let tag_offsets: Vec<_> = tag.iter().map(|t| builder.create_string(t)).collect();
        let tag_vector = builder.create_vector(&tag_offsets);
        description_tags_offsets.push(tag_vector);
    }
    let description_tags_vector = builder.create_vector(&description_tags_offsets);

    let zap_request_args = fb::ZapRequestArgs {
        kind: parsed.description.kind,
        pubkey: Some(description_pubkey),
        content: Some(description_content),
        tags: Some({
            let mut string_vec_offsets = Vec::new();
            for tag in &parsed.description.tags.0 {
                let tag_strings: Vec<_> = tag.iter().map(|t| builder.create_string(t)).collect();
                let tag_vector = builder.create_vector(&tag_strings);
                let string_vec = fb::StringVec::create(
                    builder,
                    &fb::StringVecArgs {
                        items: Some(tag_vector),
                    },
                );
                string_vec_offsets.push(string_vec);
            }
            builder.create_vector(&string_vec_offsets)
        }),
        signature: description_signature,
    };
    let zap_request_offset = fb::ZapRequest::create(builder, &zap_request_args);

    let args = fb::Kind9735ParsedArgs {
        id: Some(id),
        amount: parsed.amount,
        content: Some(content),
        bolt11: Some(bolt11),
        preimage,
        sender: Some(sender),
        recipient: Some(recipient),
        event,
        event_coordinate,
        timestamp: parsed.timestamp,
        valid: parsed.valid,
        description: Some(zap_request_offset),
    };

    let offset = fb::Kind9735Parsed::create(builder, &args);

    Ok(offset)
}

// Helper function to find a tag by name in a vec of vec of strings
fn find_tag_in_vec<'a>(tags: &'a [Vec<String>], name: &str) -> Option<&'a Vec<String>> {
    tags.iter().find(|tag| !tag.is_empty() && tag[0] == name)
}
