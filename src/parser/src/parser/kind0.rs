use crate::parser::Parser;
use crate::parser::{ParserError, Result};
use crate::utils::json::BaseJsonParser;
use rustc_hash::FxHashMap;
use shared::generated::nostr::*;
use shared::types::network::Request;
use shared::types::nostr::Event;

pub struct Kind0Parsed {
    pub pubkey: String,
    pub name: Option<String>,
    pub display_name: Option<String>,
    pub picture: Option<String>,
    pub banner: Option<String>,
    pub about: Option<String>,
    pub website: Option<String>,
    pub nip05: Option<String>,
    pub lud06: Option<String>,
    pub lud16: Option<String>,
    pub github: Option<String>,
    pub twitter: Option<String>,
    pub mastodon: Option<String>,
    pub nostr: Option<String>,

    // Alternative formats
    pub display_name_alt: Option<String>,
    pub username: Option<String>,
    pub bio: Option<String>,
    pub image: Option<String>,
    pub avatar: Option<String>,
    pub background: Option<String>,
}

impl Kind0Parsed {
    #[inline(always)]
    fn parse_profile_json(json_str: &str) -> Result<Kind0Parsed> {
        let trimmed = json_str.trim();

        // Check if the entire JSON is wrapped in quotes (stringified JSON)
        if trimmed.starts_with('"') && trimmed.ends_with('"') {
            // Parse the outer string to get the inner JSON
            let mut parser = BaseJsonParser::new(trimmed.as_bytes());
            let inner_json = parser.parse_string_unescaped()?;
            // Now parse the inner JSON
            let inner_parser = ProfileJsonParser::new(inner_json.as_bytes());
            inner_parser.parse()
        } else {
            let parser = ProfileJsonParser::new(json_str.as_bytes());
            parser.parse()
        }
    }
}

enum ProfileJsonParserData<'a> {
    Borrowed(&'a [u8]),
    Owned(Vec<u8>),
}

struct ProfileJsonParser<'a> {
    data: ProfileJsonParserData<'a>,
}

impl<'a> ProfileJsonParser<'a> {
    #[inline(always)]
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            data: ProfileJsonParserData::Borrowed(bytes),
        }
    }

    #[inline(always)]
    fn parse(mut self) -> Result<Kind0Parsed> {
        // Get the bytes to parse
        let bytes = match &self.data {
            ProfileJsonParserData::Borrowed(b) => *b,
            ProfileJsonParserData::Owned(v) => v.as_slice(),
        };

        // Handle escaped JSON if needed
        let mut parser = if let Some(unescaped) = BaseJsonParser::unescape_if_needed(bytes)? {
            // Use the unescaped data
            self.data = ProfileJsonParserData::Owned(unescaped);
            match &self.data {
                ProfileJsonParserData::Owned(v) => BaseJsonParser::new(v.as_slice()),
                _ => unreachable!(),
            }
        } else {
            BaseJsonParser::new(bytes)
        };

        parser.skip_whitespace();
        parser.expect_byte(b'{')?;

        let mut profile = Kind0Parsed {
            pubkey: "".to_string(),
            name: None,
            display_name: None,
            picture: None,
            banner: None,
            website: None,
            about: None,
            nip05: None,
            lud06: None,
            lud16: None,
            github: None,
            twitter: None,
            mastodon: None,
            nostr: None,
            display_name_alt: None,
            username: None,
            bio: None,
            image: None,
            avatar: None,
            background: None,
        };

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

            // Parse the value based on its type
            let str_value = match parser.peek() {
                b'"' => {
                    let value = parser.parse_string_unescaped()?;
                    if value.is_empty() {
                        None
                    } else {
                        Some(value)
                    }
                }
                b'{' | b'[' => {
                    // Skip complex objects and arrays - we don't need them for profile fields
                    parser.skip_value()?;
                    None
                }
                b'n' => {
                    // Handle null values
                    parser.skip_null()?;
                    None
                }
                b't' | b'f' => {
                    // Handle boolean values - convert to string
                    if parser.bytes[parser.pos..].starts_with(b"true") {
                        parser.pos += 4;
                        Some("true".to_string())
                    } else if parser.bytes[parser.pos..].starts_with(b"false") {
                        parser.pos += 5;
                        Some("false".to_string())
                    } else {
                        parser.skip_value()?;
                        None
                    }
                }
                b'0'..=b'9' | b'-' => {
                    // Handle numbers - convert to string
                    let start = parser.pos;
                    parser.skip_number()?;
                    let num_str =
                        unsafe { std::str::from_utf8_unchecked(&parser.bytes[start..parser.pos]) };
                    Some(num_str.to_string())
                }
                _ => {
                    // Unknown value type, skip it
                    parser.skip_value()?;
                    None
                }
            };

            // Match field names to profile fields
            match key {
                "name" => profile.name = str_value,
                "display_name" => profile.display_name = str_value,
                "displayName" => profile.display_name_alt = str_value,
                "username" => profile.username = str_value,
                "picture" => profile.picture = str_value,
                "image" => profile.image = str_value,
                "avatar" => profile.avatar = str_value,
                "banner" => profile.banner = str_value,
                "background" => profile.background = str_value,
                "about" => profile.about = str_value,
                "bio" => profile.bio = str_value,
                "website" => profile.website = str_value,
                "nip05" => profile.nip05 = str_value,
                "lud06" => profile.lud06 = str_value,
                "lud16" => profile.lud16 = str_value,
                "github" => profile.github = str_value,
                "twitter" => profile.twitter = str_value,
                "mastodon" => profile.mastodon = str_value,
                "nostr" => profile.nostr = str_value,
                // Skip any unknown fields including nested objects like "profileEvent"
                _ => {}
            }

            parser.skip_comma_or_end()?;
        }

        Ok(profile)
    }
}

pub struct ProfilePointer {
    pub pubkey: String,
    pub relays: Vec<String>,
}

pub struct Nip05Response {
    pub names: FxHashMap<String, String>,
    pub relays: Option<FxHashMap<String, Vec<String>>>,
}

impl Parser {
    pub fn parse_kind_0(&self, event: &Event) -> Result<(Kind0Parsed, Option<Vec<Request>>)> {
        if event.kind != 0 {
            return Err(ParserError::Other("event is not kind 0".to_string()));
        }

        let mut profile = Kind0Parsed {
            pubkey: event.pubkey.to_hex(),
            name: None,
            display_name: None,
            picture: None,
            banner: None,
            about: None,
            website: None,
            nip05: None,
            lud06: None,
            lud16: None,
            github: None,
            twitter: None,
            mastodon: None,
            nostr: None,
            display_name_alt: None,
            username: None,
            bio: None,
            image: None,
            avatar: None,
            background: None,
        };

        // Parse the content JSON
        if !event.content.is_empty() {
            match Kind0Parsed::parse_profile_json(&event.content) {
                Ok(parsed_profile) => {
                    profile = parsed_profile;
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to parse profile JSON for event {}: {}",
                        event.content(),
                        e
                    );
                }
            }
        }

        // Fallback logic: if name is empty but display_name is present, use display_name as name
        if profile.name.is_none() {
            if let Some(ref display_name) = profile.display_name {
                profile.name = Some(display_name.clone());
            } else if let Some(ref display_name_alt) = profile.display_name_alt {
                profile.name = Some(display_name_alt.clone());
            }
        }
        profile.pubkey = event.pubkey.to_hex();
        // Note: NIP-05 verification is commented out in the original Go code
        // because synchronous HTTP requests can cause deadlocks
        // We would need to implement async verification separately

        Ok((profile, None))
    }
}

// NEW: Build the FlatBuffer for Kind0Parsed
// This produces the serialized bytes ready for use in ParsedEvent's 'parsed' field.
// Adjust field mappings and args to match your exact kind0.fbs schema.
pub fn build_flatbuffer<'a, A: flatbuffers::Allocator + 'a>(
    parsed: &Kind0Parsed,
    builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
) -> Result<flatbuffers::WIPOffset<fb::Kind0Parsed<'a>>> {
    // Create string offsets for optional fields (None becomes default/empty in schema)
    let pubkey = builder.create_string(&parsed.pubkey);
    let name = parsed.name.as_ref().map(|s| builder.create_string(s));
    let display_name = parsed
        .display_name
        .as_ref()
        .map(|s| builder.create_string(s));
    let picture = parsed.picture.as_ref().map(|s| builder.create_string(s));
    let banner = parsed.banner.as_ref().map(|s| builder.create_string(s));
    let about = parsed.about.as_ref().map(|s| builder.create_string(s));
    let website = parsed.website.as_ref().map(|s| builder.create_string(s));
    let nip05 = parsed.nip05.as_ref().map(|s| builder.create_string(s));
    let lud06 = parsed.lud06.as_ref().map(|s| builder.create_string(s));
    let lud16 = parsed.lud16.as_ref().map(|s| builder.create_string(s));
    let github = parsed.github.as_ref().map(|s| builder.create_string(s));
    let twitter = parsed.twitter.as_ref().map(|s| builder.create_string(s));
    let mastodon = parsed.mastodon.as_ref().map(|s| builder.create_string(s));
    let nostr = parsed.nostr.as_ref().map(|s| builder.create_string(s));
    let display_name_alt = parsed
        .display_name_alt
        .as_ref()
        .map(|s| builder.create_string(s));
    let username = parsed.username.as_ref().map(|s| builder.create_string(s));
    let bio = parsed.bio.as_ref().map(|s| builder.create_string(s));
    let image = parsed.image.as_ref().map(|s| builder.create_string(s));
    let avatar = parsed.avatar.as_ref().map(|s| builder.create_string(s));
    let background = parsed.background.as_ref().map(|s| builder.create_string(s));

    let args = fb::Kind0ParsedArgs {
        pubkey: Some(pubkey),
        name,
        display_name,
        picture,
        banner,
        about,
        website,
        nip05,
        lud06,
        lud16,
        github,
        twitter,
        mastodon,
        nostr,
        display_name_alt,
        username,
        bio,
        image,
        avatar,
        background,
    };

    let offset = fb::Kind0Parsed::create(builder, &args);

    Ok(offset)
}
