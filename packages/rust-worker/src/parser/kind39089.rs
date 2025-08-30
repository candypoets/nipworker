use crate::parser::Parser;
use crate::types::network::Request;
use anyhow::{anyhow, Result};
use nostr::Event;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// NEW: Imports for FlatBuffers
use crate::generated::nostr::*;
use flatbuffers::FlatBufferBuilder;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Kind39089Parsed {
    #[serde(rename = "list_identifier")]
    pub list_identifier: String,
    pub people: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
}

impl Parser {
    pub fn parse_kind_39089(
        &self,
        event: &Event,
    ) -> Result<(Kind39089Parsed, Option<Vec<Request>>)> {
        if event.kind.as_u64() != 39089 {
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
            let tag_vec = tag.as_vec();
            if tag_vec.len() >= 2 && tag_vec[0] == "d" {
                result.list_identifier = tag_vec[1].clone();
                break;
            }
        }

        if result.list_identifier.is_empty() {
            return Err(anyhow!("missing required 'd' tag for list identifier"));
        }

        // Extract people from p tags
        for tag in &event.tags {
            let tag_vec = tag.as_vec();
            if tag_vec.len() >= 2 && tag_vec[0] == "p" {
                result.people.push(tag_vec[1].clone());
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
            let tag_vec = tag.as_vec();
            if tag_vec.len() >= 2 {
                match tag_vec[0].as_str() {
                    "title" => {
                        if result.title.is_none() {
                            result.title = Some(tag_vec[1].clone());
                        }
                    }
                    "description" => {
                        if result.description.is_none() {
                            result.description = Some(tag_vec[1].clone());
                        }
                    }
                    "image" => {
                        if result.image.is_none() {
                            result.image = Some(tag_vec[1].clone());
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

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind, Tag};

    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn test_parse_kind_39089_basic() {
        let keys = Keys::generate();
        let list_id = "my-friends";
        let person1 = "npub1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
        let person2 = "npub2345678901bcdef2345678901bcdef2345678901bcdef2345678901bcdef2";

        let tags = vec![
            Tag::parse(vec!["d".to_string(), list_id.to_string()]).unwrap(),
            Tag::parse(vec!["p".to_string(), person1.to_string()]).unwrap(),
            Tag::parse(vec!["p".to_string(), person2.to_string()]).unwrap(),
        ];

        let event = EventBuilder::new(Kind::Custom(39089), "", tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, requests) = parser.parse_kind_39089(&event).unwrap();

        assert_eq!(parsed.list_identifier, list_id);
        assert_eq!(parsed.people.len(), 2);
        assert!(parsed.people.contains(&person1.to_string()));
        assert!(parsed.people.contains(&person2.to_string()));
        assert!(requests.is_some());
        assert!(!requests.unwrap().is_empty());
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn test_parse_kind_39089_with_metadata_tags() {
        let keys = Keys::generate();
        let list_id = "bitcoin-devs";
        let title = "Bitcoin Developers";
        let description = "List of Bitcoin developers";
        let image = "https://example.com/bitcoin.png";

        let tags = vec![
            Tag::parse(vec!["d".to_string(), list_id.to_string()]).unwrap(),
            Tag::parse(vec!["title".to_string(), title.to_string()]).unwrap(),
            Tag::parse(vec!["description".to_string(), description.to_string()]).unwrap(),
            Tag::parse(vec!["image".to_string(), image.to_string()]).unwrap(),
        ];

        let event = EventBuilder::new(Kind::Custom(39089), "", tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, _) = parser.parse_kind_39089(&event).unwrap();

        assert_eq!(parsed.list_identifier, list_id);
        assert_eq!(parsed.title, Some(title.to_string()));
        assert_eq!(parsed.description, Some(description.to_string()));
        assert_eq!(parsed.image, Some(image.to_string()));
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn test_parse_kind_39089_with_content_metadata() {
        let keys = Keys::generate();
        let list_id = "nostr-apps";
        let content = r#"{"title":"Nostr Apps","description":"Cool Nostr applications","image":"https://example.com/nostr.png"}"#;

        let tags = vec![Tag::parse(vec!["d".to_string(), list_id.to_string()]).unwrap()];

        let event = EventBuilder::new(Kind::Custom(39089), content, tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, _) = parser.parse_kind_39089(&event).unwrap();

        assert_eq!(parsed.list_identifier, list_id);
        assert_eq!(parsed.title, Some("Nostr Apps".to_string()));
        assert_eq!(
            parsed.description,
            Some("Cool Nostr applications".to_string())
        );
        assert_eq!(
            parsed.image,
            Some("https://example.com/nostr.png".to_string())
        );
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn test_parse_kind_39089_metadata_priority() {
        let keys = Keys::generate();
        let list_id = "test-list";
        let tag_title = "Title from tag";
        let content_title = "Title from content";
        let content = format!(r#"{{"title":"{}"}}"#, content_title);

        let tags = vec![
            Tag::parse(vec!["d".to_string(), list_id.to_string()]).unwrap(),
            Tag::parse(vec!["title".to_string(), tag_title.to_string()]).unwrap(), // Tag should take priority
        ];

        let event = EventBuilder::new(Kind::Custom(39089), &content, tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, _) = parser.parse_kind_39089(&event).unwrap();

        // Content is parsed first, but tag should not override if already set
        assert_eq!(parsed.title, Some(content_title.to_string()));
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn test_parse_kind_39089_missing_d_tag() {
        let keys = Keys::generate();
        let person = "npub1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";

        let tags = vec![
            Tag::parse(vec!["p".to_string(), person.to_string()]).unwrap(),
            // Missing d tag
        ];

        let event = EventBuilder::new(Kind::Custom(39089), "", tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let result = parser.parse_kind_39089(&event);

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("missing required 'd' tag"));
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn test_parse_kind_39089_empty_list() {
        let keys = Keys::generate();
        let list_id = "empty-list";

        let tags = vec![Tag::parse(vec!["d".to_string(), list_id.to_string()]).unwrap()];

        let event = EventBuilder::new(Kind::Custom(39089), "", tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, requests) = parser.parse_kind_39089(&event).unwrap();

        assert_eq!(parsed.list_identifier, list_id);
        assert!(parsed.people.is_empty());
        assert!(requests.is_some());
        assert!(requests.unwrap().is_empty()); // No requests if no people
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn test_parse_wrong_kind() {
        let keys = Keys::generate();

        let event = EventBuilder::new(Kind::TextNote, "test", Vec::new())
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let result = parser.parse_kind_39089(&event);

        assert!(result.is_err());
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn test_parse_kind_39089_invalid_json_content() {
        let keys = Keys::generate();
        let list_id = "test-list";
        let invalid_content = r#"{"title":"unclosed"#;

        let tags = vec![Tag::parse(vec!["d".to_string(), list_id.to_string()]).unwrap()];

        let event = EventBuilder::new(Kind::Custom(39089), invalid_content, tags)
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, _) = parser.parse_kind_39089(&event).unwrap();

        // Should still work, just ignore invalid JSON content
        assert_eq!(parsed.list_identifier, list_id);
        assert!(parsed.title.is_none());
    }
}
