use crate::generated::nostr::fb::{
	ConnectionStatus, ConnectionStatusArgs, Eoce, EoceArgs, Message, MessageType, Raw, RawArgs,
	WorkerMessage, WorkerMessageArgs,
};
use crate::utils::extract_first_three;
use serde_json::Value;

fn unquote_simple(s: &str) -> &str {
	let b = s.as_bytes();
	if b.len() >= 2 && b[0] == b'"' && b[b.len() - 1] == b'"' {
		&s[1..b.len() - 1]
	} else {
		s
	}
}

fn value_to_text(v: &Value) -> String {
	v.as_str()
		.map(ToString::to_string)
		.unwrap_or_else(|| v.to_string())
}

fn parse_ok_status_reason(raw_msg: &str) -> (String, Option<String>) {
	// Strict JSON path: ["OK", <id>, <accepted>, <reason?>]
	if let Ok(Value::Array(arr)) = serde_json::from_str::<Value>(raw_msg) {
		let accepted = arr
			.get(2)
			.map(value_to_text)
			.unwrap_or_else(|| "false".to_string());
		let reason = arr.get(3).map(value_to_text);
		return (accepted, reason);
	}

	// Tolerant path for synthetic/loose frames: only first 3 tokens are available.
	// We can still extract accepted (3rd token), but not a reliable 4th token reason.
	if let Some([_k, _second, third]) = extract_first_three(raw_msg) {
		let accepted = third
			.map(|s| unquote_simple(s).to_string())
			.unwrap_or_else(|| "false".to_string());
		return (accepted, None);
	}

	("false".to_string(), None)
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
	if let Ok(Value::Array(arr)) = serde_json::from_str::<Value>(raw_msg) {
		if arr.len() < 2 {
			return None;
		}

		let kind = arr.get(0)?.as_str()?.to_string();

		let sub_id = match kind.as_str() {
			"EVENT" | "EOSE" | "OK" | "CLOSED" => {
				arr.get(1).and_then(|v| v.as_str()).map(ToString::to_string)
			}
			_ => None,
		};

		let raw_payload = match kind.as_str() {
			// ["EVENT", <subid>, <event>]
			"EVENT" => arr.get(2).map(|v| v.to_string()),
			// ["EOSE", <subid>]
			"EOSE" => None,
			// ["OK", <event_id>, <accepted>, <reason>] or synthetic ["OK", <id>, "SENT"]
			// Keep accepted token in payload; reason is handled separately when building ConnectionStatus.
			"OK" => arr.get(2).map(value_to_text),
			// ["NOTICE", <message>]
			"NOTICE" => arr.get(1).map(|v| {
				v.as_str()
					.map(ToString::to_string)
					.unwrap_or_else(|| v.to_string())
			}),
			// ["AUTH", <challenge>]
			"AUTH" => arr.get(1).map(|v| {
				v.as_str()
					.map(ToString::to_string)
					.unwrap_or_else(|| v.to_string())
			}),
			// ["CLOSED", <subid>, <msg>]
			"CLOSED" => arr.get(2).map(|v| v.to_string()),
			_ => None,
		};

		let is_eose = kind == "EOSE";
		let success = if kind == "OK" {
			match (arr.get(2), arr.get(3)) {
				// Real relay OK
				(Some(Value::Bool(accepted)), _) => *accepted,
				// Synthetic statuses
				(Some(Value::String(s)), _) => {
					s == "SUBSCRIBED" || s == "SENT" || s == "CLOSED" || s == "OK"
				}
				(_, Some(Value::Bool(accepted))) => *accepted,
				_ => false,
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

	// Fallback: tolerant first-3-elements extractor
	let Some([k_opt, second_opt, third_opt]) = extract_first_three(raw_msg) else {
		return None;
	};

	let kind_s = k_opt.map(unquote_simple)?.to_string();

	let (sub_id, raw_payload) = match kind_s.as_str() {
		"AUTH" | "NOTICE" => (None, second_opt.map(|s| unquote_simple(s).to_string())),
		"EVENT" | "EOSE" | "OK" | "CLOSED" => (
			second_opt.map(|s| unquote_simple(s).to_string()),
			third_opt.map(|s| s.to_string()),
		),
		_ => (
			second_opt.map(|s| unquote_simple(s).to_string()),
			third_opt.map(|s| s.to_string()),
		),
	};

	let is_eose = kind_s == "EOSE";
	let success = if kind_s == "OK" {
		if let Some(ref p) = raw_payload {
			let t = unquote_simple(p);
			t == "SUBSCRIBED" || t == "SENT" || t == "CLOSED" || t == "OK" || t == "true"
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
	let kind = parsed.as_ref().map(|r| r.kind.as_str()).unwrap_or("RAW");

	match kind {
		// NOTICE, AUTH, CLOSED, EOSE, OK -> ConnectionStatus
		"NOTICE" | "AUTH" | "CLOSED" | "EOSE" | "OK" => {
			let (status_text, message_text) = if kind == "OK" {
				// For OK, rewrite semantics:
				// status = accepted (3rd item), message = reason (4th item)
				parse_ok_status_reason(raw_line)
			} else {
				(
					kind.to_string(),
					parsed.as_ref().and_then(|r| r.raw_payload.clone()),
				)
			};

			let status_off = fbb.create_string(&status_text);
			let message_off = message_text.as_ref().map(|m| fbb.create_string(m));

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
				.and_then(|r| r.raw_payload.as_ref())
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
	let message_off = if message.is_empty() { None } else { Some(builder.create_string(message)) };
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
