use crate::{parser::Parser, types::network::Request};
use anyhow::{anyhow, Result};
use nostr::Event;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Kind0Parsed {
    pub pubkey: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub picture: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub banner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub about: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub website: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nip05: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lud06: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lud16: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub github: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub twitter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mastodon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nostr: Option<String>,

    // Alternative formats
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name_alt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bio: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfilePointer {
    pub pubkey: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub relays: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Nip05Response {
    pub names: HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relays: Option<HashMap<String, Vec<String>>>,
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
