use crate::generated::nostr::fb::{
    ConnectionStatus, ConnectionStatusArgs, Eoce, EoceArgs, Message, MessageType, Raw, RawArgs,
    WorkerMessage, WorkerMessageArgs,
};
use crate::transport::frame_scan::scan_relay_frame;

// ["OK", <id>, <accepted>, <reason?>] -> (accepted, reason) as borrowed slices.
// `accepted` is the unquoted string value or the raw bool/number token ("false" when missing).
fn parse_ok_status_reason(raw_msg: &str) -> (&str, Option<&str>) {
    if let Some(scan) = scan_relay_frame(raw_msg) {
        let accepted = scan.args[1].map(|v| v.inner()).unwrap_or("false");
        let reason = scan.args[2].map(|v| v.inner());
        return (accepted, reason);
    }
    ("false", None)
}

// Minimal struct for relay response (borrowed slices of the raw JSON string)
#[derive(Debug)]
pub struct RelayResponse<'a> {
    pub kind: &'a str,
    pub sub_id: Option<&'a str>,
    pub raw_payload: Option<&'a str>, // Raw JSON slice (e.g., event object for later parsing)
    pub is_eose: bool,
    pub success: bool,
}

// Parse raw relay message (JSON array string) to RelayResponse.
// Single zero-copy scan: no serde_json DOM, no reserialization of the event object.
pub fn parse_relay_response(raw_msg: &str) -> Option<RelayResponse<'_>> {
    let scan = scan_relay_frame(raw_msg)?;
    let kind = scan.kind;

    let sub_id = match kind {
        "EVENT" | "EOSE" | "OK" | "CLOSED" => scan.args[0].and_then(|v| {
            if v.is_string {
                Some(v.inner())
            } else {
                None
            }
        }),
        _ => None,
    };

    let raw_payload = match kind {
        // ["EVENT", <subid>, <event>] — keep the event object as a raw slice
        "EVENT" => scan.args[1].map(|v| v.raw),
        // ["EOSE", <subid>]
        "EOSE" => None,
        // ["OK", <event_id>, <accepted>, <reason>] or synthetic ["OK", <id>, "SENT"]
        // Keep accepted token in payload; reason is handled separately when building ConnectionStatus.
        "OK" => scan.args[1].map(|v| v.inner()),
        // ["NOTICE", <message>] / ["AUTH", <challenge>]
        "NOTICE" | "AUTH" => scan.args[0].map(|v| v.inner()),
        // ["CLOSED", <subid>, <msg>]
        "CLOSED" => scan.args[1].map(|v| v.raw),
        _ => None,
    };

    let is_eose = kind == "EOSE";
    let success = if kind == "OK" {
        let third = scan.args[1];
        let fourth = scan.args[2];
        match (third, fourth) {
            // Real relay OK: bool accepted flag dominates
            (Some(t), _) if !t.is_string && t.raw == "true" => true,
            (Some(t), _) if !t.is_string && t.raw == "false" => false,
            // Synthetic statuses
            (Some(t), _) if t.is_string => {
                matches!(t.inner(), "SUBSCRIBED" | "SENT" | "CLOSED" | "OK")
            }
            (_, Some(f)) if !f.is_string && f.raw == "true" => true,
            _ => false,
        }
    } else {
        false
    };

    Some(RelayResponse {
        kind,
        sub_id,
        raw_payload,
        is_eose,
        success,
    })
}

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

    // Determine kind from parsed response (or fall back to RAW)
    let kind = parsed.as_ref().map(|r| r.kind).unwrap_or("RAW");

    match kind {
        // NOTICE, AUTH, CLOSED, EOSE, OK -> ConnectionStatus
        "NOTICE" | "AUTH" | "CLOSED" | "EOSE" | "OK" => {
            let (status_text, message_text): (&str, Option<&str>) = if kind == "OK" {
                // For OK, rewrite semantics:
                // status = accepted (3rd item), message = reason (4th item)
                parse_ok_status_reason(raw_line)
            } else {
                (kind, parsed.as_ref().and_then(|r| r.raw_payload))
            };

            let status_off = fbb.create_string(status_text);
            let message_off = message_text.map(|m| fbb.create_string(m));

            let cs = ConnectionStatus::create(
                fbb,
                &ConnectionStatusArgs {
                    relay_url: Some(url_off),
                    status: Some(status_off),
                    message: message_off,
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
            let raw_field_off = if let Some(off) = parsed
                .as_ref()
                .and_then(|r| r.raw_payload)
                .map(|s| fbb.create_string(s))
            {
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

pub fn serialize_connection_status(url: &str, status: &str, message: &str) -> Vec<u8> {
    let mut builder = flatbuffers::FlatBufferBuilder::new();
    let url_off = builder.create_string(url);
    let status_off = builder.create_string(status);
    let message_off = if message.is_empty() {
        None
    } else {
        Some(builder.create_string(message))
    };
    let cs = ConnectionStatus::create(
        &mut builder,
        &ConnectionStatusArgs {
            relay_url: Some(url_off),
            status: Some(status_off),
            message: message_off,
        },
    );
    let wm = WorkerMessage::create(
        &mut builder,
        &WorkerMessageArgs {
            sub_id: None,
            url: Some(url_off),
            type_: MessageType::ConnectionStatus,
            content_type: Message::ConnectionStatus,
            content: Some(cs.as_union_value()),
        },
    );
    builder.finish(wm, None);
    builder.finished_data().to_vec()
}

pub fn serialize_eoce() -> Vec<u8> {
    let mut builder = flatbuffers::FlatBufferBuilder::new();
    let sid = builder.create_string("");
    let eoce = Eoce::create(
        &mut builder,
        &EoceArgs {
            subscription_id: Some(sid),
        },
    );
    let wm = WorkerMessage::create(
        &mut builder,
        &WorkerMessageArgs {
            sub_id: None,
            url: None,
            type_: MessageType::Eoce,
            content_type: Message::Eoce,
            content: Some(eoce.as_union_value()),
        },
    );
    builder.finish(wm, None);
    builder.finished_data().to_vec()
}
