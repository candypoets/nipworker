use crate::parser::Parser;
use crate::types::network::Request;
use crate::types::nostr::Event;
use anyhow::{anyhow, Result};
use serde_json::Value;

// NEW: Imports for FlatBuffers
use crate::generated::nostr::*;
use flatbuffers::FlatBufferBuilder;

pub struct Kind39089Parsed {
    pub list_identifier: String,
    pub people: Vec<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub image: Option<String>,
}

impl Parser {
    pub fn parse_kind_39089(
        &self,
        event: &Event,
    ) -> Result<(Kind39089Parsed, Option<Vec<Request>>)> {
        if event.kind != 39089 {
            return Err(anyhow!("event is not kind 39089"));
        }

        let mut requests = Vec::new();
        let mut result = Kind39089Parsed {
            list_identifier: String::new(),
            people: Vec::new(),
            title: None,
            description: None,
            image: None,
        };

        // Find the "d" tag which contains the list identifier
        for tag in &event.tags {
            if tag.len() >= 2 && tag[0] == "d" {
                result.list_identifier = tag[1].clone();
                break;
            }
        }

        if result.list_identifier.is_empty() {
            return Err(anyhow!("missing required 'd' tag for list identifier"));
        }

        // Extract people from p tags
        for tag in &event.tags {
            if tag.len() >= 2 && tag[0] == "p" {
                result.people.push(tag[1].clone());
            }
        }

        // Parse content for metadata if present
        if !event.content.is_empty() {
            if let Ok(content_value) = serde_json::from_str::<Value>(&event.content) {
                if let Some(content_obj) = content_value.as_object() {
                    if let Some(title) = content_obj.get("title").and_then(|v| v.as_str()) {
                        result.title = Some(title.to_string());
                    }
                    if let Some(description) =
                        content_obj.get("description").and_then(|v| v.as_str())
                    {
                        result.description = Some(description.to_string());
                    }
                    if let Some(image) = content_obj.get("image").and_then(|v| v.as_str()) {
                        result.image = Some(image.to_string());
                    }
                }
            }
        }

        // Check for title, description, or image tags
        for tag in &event.tags {
            if tag.len() >= 2 {
                match tag[0].as_str() {
                    "title" => {
                        if result.title.is_none() {
                            result.title = Some(tag[1].clone());
                        }
                    }
                    "description" => {
                        if result.description.is_none() {
                            result.description = Some(tag[1].clone());
                        }
                    }
                    "image" => {
                        if result.image.is_none() {
                            result.image = Some(tag[1].clone());
                        }
                    }
                    _ => {}
                }
            }
        }

        // Request profiles for all people in the list
        if !result.people.is_empty() {
            requests.push(Request {
                authors: result.people.clone(),
                kinds: vec![0, 10002], // Profile metadata and relay lists
                relays: self.database.find_relay_candidates(0, "", &false),
                ..Default::default()
            });
        }

        Ok((result, Some(requests)))
    }
}

// NEW: Build the FlatBuffer for Kind30000Parsed (Kind39089)
pub fn build_flatbuffer<'a, A: flatbuffers::Allocator + 'a>(
    parsed: &Kind39089Parsed,
    builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
) -> Result<flatbuffers::WIPOffset<fb::Kind39089Parsed<'a>>> {
    let list_identifier = builder.create_string(&parsed.list_identifier);

    // Build people vector
    let people_offsets: Vec<_> = parsed
        .people
        .iter()
        .map(|person| builder.create_string(person))
        .collect();
    let people_vector = builder.create_vector(&people_offsets);

    let title = parsed.title.as_ref().map(|t| builder.create_string(t));
    let description = parsed
        .description
        .as_ref()
        .map(|d| builder.create_string(d));
    let image = parsed.image.as_ref().map(|i| builder.create_string(i));

    let args = fb::Kind39089ParsedArgs {
        list_identifier: Some(list_identifier),
        people: Some(people_vector),
        title,
        description,
        image,
    };

    let offset = fb::Kind39089Parsed::create(builder, &args);

    Ok(offset)
}
