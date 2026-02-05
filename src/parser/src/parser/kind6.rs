use crate::parser::{Parser, ParserError, Result};
use crate::types::parsed_event::{ParsedData, ParsedEvent};
use crate::utils::request_deduplication::RequestDeduplicator;

// NEW: Imports for FlatBuffers
use shared::generated::nostr::*;
use shared::types::network::Request;
use shared::types::{Event, REPOST, TEXT_NOTE};

/// Kind6Parsed stores the reposted event.
/// 
/// To avoid infinite recursion in the type system (ParsedEvent -> ParsedData -> Kind6Parsed -> ParsedEvent),
/// we store the nested ParsedEvent in a Box. Box provides heap indirection, breaking the recursive
/// type size calculation.
/// 
/// The FlatBuffer serialization handles this by flattening the structure - the nested ParsedEvent
/// is serialized directly into the parent's FlatBuffer without creating a Rust-level recursive type.
pub struct Kind6Parsed {
	/// The parsed reposted event, boxed to avoid infinite type recursion
	pub reposted_event: Option<Box<ParsedEvent>>,
}

impl Parser {
	pub fn parse_kind_6(&self, event: &Event) -> Result<(Kind6Parsed, Option<Vec<Request>>)> {
		let mut requests = Vec::<Request>::new();
		if event.kind() != REPOST {
			return Err(ParserError::Other("event is not kind 6".to_string()));
		}

		// Find the e tag for the reposted event (should be the last one if multiple)
		let e_tag = match event
			.tags
			.iter()
			.rev()
			.find(|tag| tag.first().map(|s| s.as_str()) == Some("e"))
		{
			Some(tag) if tag.len() >= 2 => tag,
			_ => {
				return Err(ParserError::Other(
					"repost must have at least one e tag".to_string(),
				))
			}
		};

		let event_id = e_tag[1].clone();

		// Extract relay hint if available
		let mut relay_hint = String::new();
		if e_tag.len() >= 3 {
			relay_hint = e_tag[2].clone();
		}

		// Try to parse the reposted event from content
		let mut reposted_event: Option<Box<ParsedEvent>> = None;

		if !event.content.is_empty() {
			match Event::from_json(&event.content) {
				Ok(parsed_event)
					if !parsed_event.id.to_string().is_empty()
						&& parsed_event.kind() == TEXT_NOTE =>
				{
					// Parse the event using kind1 parser
					match self.parse_kind_1(&parsed_event) {
						Ok((parsed_content, parsed_requests)) => {
							// Create a ParsedEvent with the parsed content and box it
							let parsed_event_struct = ParsedEvent {
								event: parsed_event,
								parsed: Some(ParsedData::Kind1(parsed_content)),
								relays: vec![],
								requests: Some(vec![]),
							};

							// Box the event to break type recursion
							reposted_event = Some(Box::new(parsed_event_struct));

							// Add all requests from kind1 parsing
							if let Some(reqs) = parsed_requests {
								requests.extend(reqs);
							}
						}
						_ => {}
					}
				}
				_ => {
					// Try to parse as a different format or structure if needed
				}
			}
		}

		// If we couldn't parse the content or it was empty, request the original event
		if reposted_event.is_none() {
			let mut relays = vec![];
			relays.push(relay_hint);

			requests.push(Request {
				ids: vec![event_id],
				cache_first: true,
				close_on_eose: true,
				relays,
				..Default::default()
			});
		}

		let result = Kind6Parsed { reposted_event };

		// Deduplicate requests using the utility
		let deduplicated_requests = RequestDeduplicator::deduplicate_requests(&requests);

		Ok((result, Some(deduplicated_requests)))
	}
}

/// Build the FlatBuffer for Kind6Parsed
/// 
/// This serializes the nested ParsedEvent by calling its own build_flatbuffer method.
/// The recursion is handled at serialization time, not in the Rust type system.
pub fn build_flatbuffer<'a, A: flatbuffers::Allocator + 'a>(
	parsed: &Kind6Parsed,
	builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
) -> Result<flatbuffers::WIPOffset<fb::Kind6Parsed<'a>>> {
	// Serialize the nested ParsedEvent if present
	let reposted_event = if let Some(ref boxed_event) = parsed.reposted_event {
		// Build the nested ParsedEvent into the current builder
		// We need to handle the lifetime mismatch by building fresh
		match build_nested_parsed_event(boxed_event, builder) {
			Ok(offset) => Some(offset),
			Err(_) => None,
		}
	} else {
		None
	};

	let args = fb::Kind6ParsedArgs { reposted_event };
	let offset = fb::Kind6Parsed::create(builder, &args);
	Ok(offset)
}

/// Helper to build a nested ParsedEvent into a FlatBuffer
/// 
/// This creates a fresh serialization of the nested event into the provided builder.
/// It handles the Kind1 case specifically since that's what reposts contain.
fn build_nested_parsed_event<'a, A: flatbuffers::Allocator + 'a>(
	event: &ParsedEvent,
	builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
) -> Result<flatbuffers::WIPOffset<fb::ParsedEvent<'a>>> {
	// Build the basic event fields
	let id_offset = builder.create_string(&event.event.id.to_hex());
	let pubkey_offset = builder.create_string(&event.event.pubkey.to_hex());

	// Build relays
	let relays_offset = if event.relays.is_empty() {
		None
	} else {
		let offsets: Vec<_> = event.relays.iter().map(|r| builder.create_string(r)).collect();
		Some(builder.create_vector(&offsets))
	};

	// Note: We skip serializing requests for the nested event.
	// Requests are only used for fetching related data and aren't needed
	// in the serialized output. This also avoids lifetime issues with
	// the generic allocator.
	let requests_offset = None;

	// Build tags
	let mut tag_offsets = Vec::new();
	for tag in &event.event.tags {
		let strings: Vec<_> = tag.iter().map(|t| builder.create_string(t)).collect();
		let vec = builder.create_vector(&strings);
		let string_vec = fb::StringVec::create(
			builder,
			&fb::StringVecArgs { items: Some(vec) },
		);
		tag_offsets.push(string_vec);
	}
	let tags_offset = builder.create_vector(&tag_offsets);

	// Build parsed data
	let (parsed_type, parsed_offset) = if let Some(ref parsed_data) = event.parsed {
		build_nested_parsed_data(parsed_data, builder)?
	} else {
		return Err(ParserError::Other("No parsed data".to_string()));
	};

	let args = fb::ParsedEventArgs {
		id: Some(id_offset),
		pubkey: Some(pubkey_offset),
		created_at: event.event.created_at as u32,
		kind: event.event.kind as u16,
		parsed_type,
		parsed: Some(parsed_offset),
		requests: requests_offset,
		relays: relays_offset,
		tags: Some(tags_offset),
	};

	Ok(fb::ParsedEvent::create(builder, &args))
}

/// Build nested parsed data, handling the Kind1 case specifically
fn build_nested_parsed_data<'a, A: flatbuffers::Allocator + 'a>(
	data: &ParsedData,
	builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
) -> Result<(fb::ParsedData, flatbuffers::WIPOffset<flatbuffers::UnionWIPOffset>)> {
	match data {
		ParsedData::Kind1(kind1_data) => {
			// Use the existing kind1 build_flatbuffer
			let offset = crate::parser::kind1::build_flatbuffer(kind1_data, builder)?;
			Ok((fb::ParsedData::Kind1Parsed, offset.as_union_value()))
		}
		// For other types, we'd need to add similar handling
		// For now, return an error since reposts should only contain Kind1
		_ => Err(ParserError::Other(
			format!("Unsupported nested parsed data type for repost: not Kind1")
		)),
	}
}
