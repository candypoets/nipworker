use crate::{console_log, generated::nostr::fb::OutEnvelopeBuilder, utils::extract_first_three};

use super::generated::nostr::fb::{MessageKind /* , NostrEvent, ParsedEvent */, OutEnvelope};
use flatbuffers::{FlatBufferBuilder, WIPOffset};
use serde_json::Value; // Adjust namespace if needed

fn unquote_simple(s: &str) -> &str {
    let b = s.as_bytes();
    if b.len() >= 2 && b[0] == b'"' && b[b.len() - 1] == b'"' {
        &s[1..b.len() - 1]
    } else {
        s
    }
}

// Minimal struct for relay response (extracted from raw JSON string)
#[derive(Debug)]
pub struct RelayResponse {
    pub kind: String,
    pub sub_id: Option<String>,
    pub raw_payload: Option<String>, // Stringified JSON (e.g., event object as string for later parsing)
    pub is_eose: bool,
    pub success: bool,
}

// Parse raw relay message (JSON array string) to RelayResponse
pub fn parse_relay_response(raw_msg: &str) -> Option<RelayResponse> {
    // First try strict JSON (best case)
    if let Ok(parsed) = serde_json::from_str::<Value>(raw_msg) {
        if let Value::Array(arr) = parsed {
            if arr.len() < 2 {
                return None;
            }
            let kind = arr[0].as_str()?.to_string();
            let sub_id = arr.get(1)?.as_str().map(ToString::to_string);

            let raw_payload = match kind.as_str() {
                "EVENT" | "OK" | "NOTICE" | "AUTH" | "CLOSED" => arr.get(2).map(|v| v.to_string()),
                "EOSE" => None,
                _ => None,
            };

            let is_eose = kind == "EOSE";
            // Standard OK: ["OK", sub_id, event_id, bool]
            let success = if kind == "OK" {
                if arr.len() > 3 {
                    arr[2].as_str() == Some("OK") && arr[3].as_bool().unwrap_or(false)
                } else if arr.len() == 3 && arr[2].as_str() == Some("SUBSCRIBED") {
                    true
                } else {
                    false
                }
            } else {
                false
            };

            return Some(RelayResponse {
                kind,
                sub_id,
                raw_payload,
                is_eose,
                success,
            });
        }
        // Not an array -> fall through to tolerant path
    }

    // Fallback: tolerant first-3-elements extractor
    // Handles tokens like SUBSCRIBED without quotes, strings, or objects.
    let Some([k_opt, sid_opt, payload_opt]) = extract_first_three(raw_msg) else {
        return None;
    };

    // Kind must be a string token; unquote
    let kind_s = k_opt.map(unquote_simple)?.to_string();

    // Sub id might be empty string ("") — keep as Some("") to route globally later if you wish
    let sub_id = sid_opt.map(|s| unquote_simple(s).to_string());

    // Third token: may be object, string, or primitive (like SUBSCRIBED)
    let raw_payload = payload_opt.map(|s| s.to_string());

    let is_eose = kind_s == "EOSE";
    let success = if kind_s == "OK" {
        // Tolerant: treat third token SUBSCRIBED (quoted or bare) as success=true
        if let Some(ref p) = raw_payload {
            let t = unquote_simple(p);
            t == "SUBSCRIBED"
        } else {
            false
        }
    } else {
        false
    };

    Some(RelayResponse {
        kind: kind_s,
        sub_id,
        raw_payload,
        is_eose,
        success,
    })
}

// Build OutEnvelope (minimal: raw_payload as string in message; skip structured event/parsed_event)
fn build_out_envelope<'a>(
    fbb: &mut FlatBufferBuilder<'a>,
    sub_id: &str,
    url: &str,
    response: &RelayResponse,
) -> WIPOffset<OutEnvelope<'a>> {
    let sub_id_offset = fbb.create_string(sub_id);
    let url_offset = fbb.create_string(url);
    let kind_offset = fbb.create_string(&response.kind);
    let message_offset = response.raw_payload.as_ref().map(|p| fbb.create_string(p));

    let mut builder = OutEnvelopeBuilder::new(fbb);
    builder.add_sub_id(sub_id_offset);
    builder.add_url(url_offset);
    builder.add_kind(kind_offset);
    builder.add_is_eose(response.is_eose);
    // Skip event/parsed_event (leave as null—main worker parses message for EVENT)
    if let Some(offset) = message_offset {
        builder.add_message(offset);
    }
    builder.add_success(response.success);
    builder.finish()
}

// Serialize to prefixed bytes: [u32_be len][fb_bytes]
// Returns Some(Vec<u8>) if valid, None if parse fails
pub fn serialize_out_envelope(
    sub_id: &str, // Caller sub_id (for fallback routing)
    url: &str,
    raw: &str, // Raw relay JSON string
) -> Option<Vec<u8>> {
    // Return prefixed bytes + envelope_sub_id (for caller routing if needed)
    parse_relay_response(raw).map(|response| {
        // Prefer response.sub_id for envelope (accurate from raw, even empty)
        let envelope_sub_id = response.sub_id.as_ref().cloned().unwrap_or_else(|| sub_id.to_string());  // Fallback to caller if None
        // Lenient for global: Always build if parse succeeded
        let is_global = ["AUTH", "NOTICE", "CLOSED", "OK"].contains(&response.kind.as_str()) && response.sub_id.as_ref().map_or(true, |id| id.is_empty());
        if is_global {
            console_log!("Global message ({}): using envelope_sub_id='{}' (response='{:?}', caller='{}')", response.kind, envelope_sub_id, response.sub_id, sub_id);  // Debug: Global
        } else if response.sub_id.as_ref().map_or(false, |id| id != sub_id) {
            console_log!("sub_id mismatch: envelope='{}', caller='{}' for kind={}, raw_len={}", envelope_sub_id, sub_id, response.kind, raw.len());  // Error: Mismatch
            return (vec![], envelope_sub_id);  // Still build but empty (or None if strict)
        }
        // Build FB with envelope_sub_id
        let mut fbb = FlatBufferBuilder::new();
        let envelope_offset = build_out_envelope(&mut fbb, &envelope_sub_id, url, &response);
        fbb.finish(envelope_offset, None);

        let fb_bytes = fbb.finished_data().to_vec();
        if fb_bytes.is_empty() {
            console_log!("FB build failed: empty bytes for sub_id={}, kind={}", envelope_sub_id, response.kind);  // Error: Empty FB
            return (vec![], envelope_sub_id);
        }
        let mut prefixed = Vec::with_capacity(4 + fb_bytes.len());
        prefixed.extend_from_slice(&(fb_bytes.len() as u32).to_be_bytes());
        prefixed.extend_from_slice(&fb_bytes);
        (prefixed, envelope_sub_id)  // Return bytes + used sub_id for caller
    }).and_then(|(bytes, _)| if !bytes.is_empty() { Some(bytes) } else { None })
    // Filter empty
}
