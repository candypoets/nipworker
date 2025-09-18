use crate::parser::{ParserError, Result};
use crate::types::nostr::Event;
use crate::{generated::nostr::*, parser::Parser, types::network::Request};
use rustc_hash::FxHashMap;

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
        // Unescape the entire JSON string first
        let unescaped = Self::unescape_json(json_str);
        let parser = ProfileJsonParser::new(unescaped.as_bytes());
        parser.parse()
    }

    #[inline(always)]
    fn unescape_json(input: &str) -> String {
        let mut result = String::with_capacity(input.len());
        let mut chars = input.chars().peekable();

        while let Some(ch) = chars.next() {
            if ch == '\\' {
                if let Some(&next_ch) = chars.peek() {
                    match next_ch {
                        '"' | '\\' | '/' | '{' | '}' | '[' | ']' => {
                            chars.next(); // consume the escaped char
                            result.push(next_ch);
                        }
                        _ => {
                            // Keep the backslash for other escape sequences that will be handled during parsing
                            result.push(ch);
                        }
                    }
                } else {
                    result.push(ch);
                }
            } else {
                result.push(ch);
            }
        }

        result
    }
}

struct ProfileJsonParser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> ProfileJsonParser<'a> {
    #[inline(always)]
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    #[inline(always)]
    fn parse(mut self) -> Result<Kind0Parsed> {
        self.skip_whitespace();
        self.expect_byte(b'{')?;

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

        while self.pos < self.bytes.len() {
            self.skip_whitespace();
            if self.peek() == b'}' {
                self.pos += 1;
                break;
            }

            let key = self.parse_string()?;
            self.skip_whitespace();
            self.expect_byte(b':')?;
            self.skip_whitespace();

            let value = self.parse_string()?;
            let str_value = if value.is_empty() {
                None
            } else {
                Some(value.to_string())
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
                _ => {} // Ignore unknown fields
            }

            self.skip_comma_or_end()?;
        }

        Ok(profile)
    }

    #[inline(always)]
    fn peek(&self) -> u8 {
        self.bytes[self.pos]
    }

    #[inline(always)]
    fn skip_whitespace(&mut self) {
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    #[inline(always)]
    fn expect_byte(&mut self, expected: u8) -> Result<()> {
        if self.pos >= self.bytes.len() || self.bytes[self.pos] != expected {
            return Err(ParserError::InvalidFormat(
                "Unexpected byte in profile JSON".to_string(),
            ));
        }
        self.pos += 1;
        Ok(())
    }

    #[inline(always)]
    fn parse_string(&mut self) -> Result<&'a str> {
        self.expect_byte(b'"')?;
        let start = self.pos;

        // Find the end quote, handling escaped quotes
        while self.pos < self.bytes.len() {
            match self.bytes[self.pos] {
                b'"' => {
                    let result =
                        unsafe { std::str::from_utf8_unchecked(&self.bytes[start..self.pos]) };
                    self.pos += 1;
                    return Ok(result);
                }
                b'\\' => {
                    // Skip escaped character (simple handling for common escapes)
                    if self.pos + 1 < self.bytes.len() {
                        self.pos += 2; // Skip \ and next char
                    } else {
                        self.pos += 1;
                    }
                }
                _ => self.pos += 1,
            }
        }

        Err(ParserError::InvalidFormat(
            "Unterminated string in profile JSON".to_string(),
        ))
    }

    #[inline(always)]
    fn skip_comma_or_end(&mut self) -> Result<()> {
        self.skip_whitespace();
        if self.pos < self.bytes.len() && self.bytes[self.pos] == b',' {
            self.pos += 1;
        }
        Ok(())
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
