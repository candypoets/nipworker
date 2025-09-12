use crate::{generated::nostr::*, parser::Parser, types::network::Request};
use anyhow::{anyhow, Result};
use flatbuffers::FlatBufferBuilder;
use nostr::Event;
use rustc_hash::FxHashMap;
use serde_json::Value;

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
        if event.kind.as_u64() != 0 {
            return Err(anyhow!("event is not kind 0"));
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
            if let Ok(content_value) = serde_json::from_str::<Value>(&event.content) {
                if let Some(content_obj) = content_value.as_object() {
                    for (key, value) in content_obj {
                        if let Some(str_value) = value.as_str() {
                            let str_value = if str_value.is_empty() {
                                None
                            } else {
                                Some(str_value.to_string())
                            };

                            match key.as_str() {
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
                        }
                    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind};

    #[test]
    fn test_parse_kind_0_basic() {
        let keys = Keys::generate();
        let content = r#"{"name":"Alice","about":"Bitcoin enthusiast","picture":"https://example.com/pic.jpg"}"#;

        let event = EventBuilder::new(Kind::Metadata, content, Vec::new())
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, _) = parser.parse_kind_0(&event).unwrap();

        assert_eq!(parsed.pubkey, keys.public_key().to_hex());
        assert_eq!(parsed.name, Some("Alice".to_string()));
        assert_eq!(parsed.about, Some("Bitcoin enthusiast".to_string()));
        assert_eq!(
            parsed.picture,
            Some("https://example.com/pic.jpg".to_string())
        );
    }

    #[test]
    fn test_parse_kind_0_alternative_fields() {
        let keys = Keys::generate();
        let content = r#"{"displayName":"Bob","bio":"Nostr developer","avatar":"https://example.com/avatar.jpg"}"#;

        let event = EventBuilder::new(Kind::Metadata, content, Vec::new())
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, _) = parser.parse_kind_0(&event).unwrap();

        assert_eq!(parsed.display_name_alt, Some("Bob".to_string()));
        assert_eq!(parsed.bio, Some("Nostr developer".to_string()));
        assert_eq!(
            parsed.avatar,
            Some("https://example.com/avatar.jpg".to_string())
        );
        // Name should fallback to displayName
        assert_eq!(parsed.name, Some("Bob".to_string()));
    }

    #[test]
    fn test_parse_kind_0_empty_content() {
        let keys = Keys::generate();

        let event = EventBuilder::new(Kind::Metadata, "", Vec::new())
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, _) = parser.parse_kind_0(&event).unwrap();

        assert_eq!(parsed.pubkey, keys.public_key().to_hex());
        assert_eq!(parsed.name, None);
        assert_eq!(parsed.about, None);
    }

    #[test]
    fn test_parse_kind_0_invalid_json() {
        let keys = Keys::generate();
        let content = r#"{"name":"Alice","invalid json"#;

        let event = EventBuilder::new(Kind::Metadata, content, Vec::new())
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let (parsed, _) = parser.parse_kind_0(&event).unwrap();

        // Should still work, just with empty fields
        assert_eq!(parsed.pubkey, keys.public_key().to_hex());
        assert_eq!(parsed.name, None);
    }

    #[test]
    fn test_parse_wrong_kind() {
        let keys = Keys::generate();

        let event = EventBuilder::new(Kind::TextNote, "test", Vec::new())
            .to_event(&keys)
            .unwrap();

        let parser = Parser::default();
        let result = parser.parse_kind_0(&event);

        assert!(result.is_err());
    }
}
