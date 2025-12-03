use crate::parser::Parser;
use crate::parser::{ParserError, Result};
use crate::types::network::Request;
use crate::types::nostr::Event;
use crate::utils::json::BaseJsonParser;
use std::fmt::Write;

// NEW: Imports for FlatBuffers
use crate::generated::nostr::*;

pub struct Kind39089Parsed {
    pub list_identifier: String,
    pub people: Vec<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub image: Option<String>,
}

impl Kind39089Parsed {
    /// Parse Kind39089Parsed from JSON string
    pub fn from_json(json: &str) -> Result<Self> {
        let parser = Kind39089ParsedParser::new(json.as_bytes());
        parser.parse()
    }

    /// Serialize Kind39089Parsed to JSON string
    pub fn to_json(&self) -> String {
        let mut result = String::with_capacity(self.calculate_json_size());

        result.push('{');

        // Required fields
        write!(
            result,
            r#""list_identifier":"{}","people":["#,
            Self::escape_string(&self.list_identifier)
        )
        .unwrap();

        for (i, person) in self.people.iter().enumerate() {
            if i > 0 {
                result.push(',');
            }
            result.push('"');
            Self::escape_string_to(&mut result, person);
            result.push('"');
        }
        result.push_str("]");

        // Optional fields
        if let Some(ref title) = self.title {
            write!(result, r#","title":"{}""#, Self::escape_string(title)).unwrap();
        }
        if let Some(ref description) = self.description {
            write!(
                result,
                r#","description":"{}""#,
                Self::escape_string(description)
            )
            .unwrap();
        }
        if let Some(ref image) = self.image {
            write!(result, r#","image":"{}""#, Self::escape_string(image)).unwrap();
        }

        result.push('}');
        result
    }

    #[inline(always)]
    fn calculate_json_size(&self) -> usize {
        let mut size = 30; // Base structure

        // Required fields
        size += self.list_identifier.len() * 2; // Escaping
        for person in &self.people {
            size += person.len() * 2 + 4; // Escaped string + quotes + comma
        }

        // Optional fields
        if let Some(ref title) = self.title {
            size += 10 + title.len() * 2; // "title":"" + escaping
        }
        if let Some(ref description) = self.description {
            size += 16 + description.len() * 2; // "description":"" + escaping
        }
        if let Some(ref image) = self.image {
            size += 10 + image.len() * 2; // "image":"" + escaping
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

impl Parser {
    pub fn parse_kind_39089(
        &self,
        event: &Event,
    ) -> Result<(Kind39089Parsed, Option<Vec<Request>>)> {
        if event.kind != 39089 {
            return Err(ParserError::Other("event is not kind 39089".to_string()));
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
            return Err(ParserError::Other(
                "missing required 'd' tag for list identifier".to_string(),
            ));
        }

        // Extract people from p tags
        for tag in &event.tags {
            if tag.len() >= 2 && tag[0] == "p" {
                result.people.push(tag[1].clone());
            }
        }

        // ✅ UPDATED: Parse content using our custom parser instead of serde_json
        if !event.content.is_empty() {
            if let Ok(content) = Kind39089Parsed::from_json(&event.content) {
                // Use the parsed content for metadata
                result.title = content.title;
                result.description = content.description;
                result.image = content.image;
            }
        }

        // Check for title, description, or image tags (fallback)
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

// Custom JSON parser for Kind39089Parsed
enum Kind39089ParsedParserData<'a> {
    Borrowed(&'a [u8]),
    Owned(Vec<u8>),
}

struct Kind39089ParsedParser<'a> {
    data: Kind39089ParsedParserData<'a>,
}

impl<'a> Kind39089ParsedParser<'a> {
    #[inline(always)]
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            data: Kind39089ParsedParserData::Borrowed(bytes),
        }
    }

    #[inline(always)]
    fn parse(mut self) -> Result<Kind39089Parsed> {
        // Get the bytes to parse
        let bytes = match &self.data {
            Kind39089ParsedParserData::Borrowed(b) => *b,
            Kind39089ParsedParserData::Owned(v) => v.as_slice(),
        };

        // Handle escaped JSON if needed
        let mut parser = if let Some(unescaped) = BaseJsonParser::unescape_if_needed(bytes)? {
            // Use the unescaped data
            self.data = Kind39089ParsedParserData::Owned(unescaped);
            match &self.data {
                Kind39089ParsedParserData::Owned(v) => BaseJsonParser::new(v.as_slice()),
                _ => unreachable!(),
            }
        } else {
            BaseJsonParser::new(bytes)
        };

        parser.skip_whitespace();
        parser.expect_byte(b'{')?;

        let mut list_identifier = String::new();
        let mut people = Vec::new();
        let mut title = None;
        let mut description = None;
        let mut image = None;

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
                "list_identifier" => list_identifier = parser.parse_string_unescaped()?,
                "people" => people = self.parse_string_array(&mut parser)?,
                "title" => title = Some(parser.parse_string_unescaped()?),
                "description" => description = Some(parser.parse_string_unescaped()?),
                "image" => image = Some(parser.parse_string_unescaped()?),
                _ => parser.skip_value()?,
            }

            parser.skip_comma_or_end()?;
        }

        if list_identifier.is_empty() {
            return Err(ParserError::InvalidFormat(
                "Missing required list_identifier field".to_string(),
            ));
        }

        Ok(Kind39089Parsed {
            list_identifier,
            people,
            title,
            description,
            image,
        })
    }

    #[inline(always)]
    fn parse_string_array(&self, parser: &mut BaseJsonParser) -> Result<Vec<String>> {
        parser.expect_byte(b'[')?;
        let mut array = Vec::new();

        while parser.pos < parser.bytes.len() {
            parser.skip_whitespace();
            if parser.peek() == b']' {
                parser.pos += 1;
                break;
            }

            let value = parser.parse_string_unescaped()?;
            array.push(value);
            parser.skip_comma_or_end()?;
        }

        Ok(array)
    }
}
