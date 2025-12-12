use crate::utils::extract_first_three;
use serde_json::Value;
use shared::generated::nostr::fb::{
    ConnectionStatus, ConnectionStatusArgs, Message, MessageType, Raw, RawArgs, WorkerMessage,
    WorkerMessageArgs,
}; // Adjust namespace if needed

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
// Build WorkerMessage for relay output
pub fn build_worker_message<'a>(
    fbb: &mut flatbuffers::FlatBufferBuilder<'a>,
    sub_id: &str,
    url: &str,
    raw_line: &str, // full raw relay line as a fallback for Raw
) -> flatbuffers::WIPOffset<WorkerMessage<'a>> {
    let sub_id_off = fbb.create_string(sub_id);
    let url_off = fbb.create_string(url);

    let parsed = parse_relay_response(raw_line);

    // Determine kind and payload
    let (kind, message_opt) = if let Some(ref r) = parsed {
        (
            r.kind.as_str(),
            r.raw_payload.as_ref().map(|s| fbb.create_string(s)),
        )
    } else {
        // If parsing fails, treat as Raw with the full line
        ("RAW", Some(fbb.create_string(raw_line)))
    };

    match kind {
        // NOTICE, AUTH, CLOSED, EOSE, OK -> ConnectionStatus
        "NOTICE" | "AUTH" | "CLOSED" | "EOSE" | "OK" => {
            let status_off = fbb.create_string(&kind);
            let cs = ConnectionStatus::create(
                fbb,
                &ConnectionStatusArgs {
                    relay_url: Some(url_off),
                    status: Some(status_off),
                    message: message_opt,
                },
            );
            WorkerMessage::create(
                fbb,
                &WorkerMessageArgs {
                    sub_id: Some(sub_id_off),
                    url: Some(url_off),
                    type_: MessageType::ConnectionStatus,
                    content_type: Message::ConnectionStatus,
                    content: Some(cs.as_union_value()),
                },
            )
        }
        // Default: Raw (json-encoded event or any raw message)
        _ => {
            // Prefer the parsed payload (EVENT array[2]); else fall back to the full raw line
            let raw_field_off = if let Some(off) = message_opt {
                off
            } else {
                fbb.create_string(raw_line)
            };
            let raw_msg = Raw::create(
                fbb,
                &RawArgs {
                    raw: Some(raw_field_off),
                },
            );

            WorkerMessage::create(
                fbb,
                &WorkerMessageArgs {
                    sub_id: Some(sub_id_off),
                    url: Some(url_off),
                    type_: MessageType::Raw,
                    content_type: Message::Raw,
                    content: Some(raw_msg.as_union_value()),
                },
            )
        }
    }
}
