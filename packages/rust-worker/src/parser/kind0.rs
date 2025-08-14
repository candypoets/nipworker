// nipworker/packages/rust-worker/src/parser/kind0.rs
use crate::generated::nostr::fb::{Kind0Parsed, Kind0ParsedArgs};

use crate::{parser::Parser, types::network::Request};
use anyhow::{anyhow, Result};
use flatbuffers::FlatBufferBuilder;
use nostr::Event;
use serde::Deserialize;

#[derive(Deserialize, Default)]
struct Kind0Content {
    name: Option<String>,
    display_name: Option<String>,
    #[serde(rename = "displayName")]
    display_name_alt: Option<String>,
    picture: Option<String>,
    banner: Option<String>,
    about: Option<String>,
    website: Option<String>,
    nip05: Option<String>,
    lud06: Option<String>,
    lud16: Option<String>,
    github: Option<String>,
    twitter: Option<String>,
    mastodon: Option<String>,
    nostr: Option<String>,
    username: Option<String>,
    bio: Option<String>,
    image: Option<String>,
    avatar: Option<String>,
    background: Option<String>,
}

impl Parser {
    pub fn parse_kind_0(&self, event: &Event) -> Result<(Vec<u8>, Vec<u8>)> {
        if event.kind.as_u64() != 0 {
            return Err(anyhow!("event is not kind 0"));
        }

        let mut builder = FlatBufferBuilder::with_capacity(1024);
        let pubkey_off = builder.create_string(&event.pubkey.to_hex());

        // Parse JSON content with serde
        let mut content: Kind0Content = if event.content.is_empty() {
            Kind0Content::default()
        } else {
            serde_json::from_str(&event.content).unwrap_or_default()
        };

        // Apply fallback logic
        if content.name.is_none() {
            content.name = content
                .display_name
                .as_ref()
                .or(content.display_name_alt.as_ref())
                .cloned();
        }

        // Helper macro to create string offsets
        macro_rules! create_offset {
            ($field:expr) => {
                $field.as_ref().map(|s| builder.create_string(s))
            };
        }

        let kind0_args = Kind0ParsedArgs {
            pubkey: Some(pubkey_off),
            name: create_offset!(content.name),
            display_name: create_offset!(content.display_name),
            display_name_alt: create_offset!(content.display_name_alt),
            picture: create_offset!(content.picture),
            banner: create_offset!(content.banner),
            about: create_offset!(content.about),
            website: create_offset!(content.website),
            nip05: create_offset!(content.nip05),
            lud06: create_offset!(content.lud06),
            lud16: create_offset!(content.lud16),
            github: create_offset!(content.github),
            twitter: create_offset!(content.twitter),
            mastodon: create_offset!(content.mastodon),
            nostr: create_offset!(content.nostr),
            username: create_offset!(content.username),
            bio: create_offset!(content.bio),
            image: create_offset!(content.image),
            avatar: create_offset!(content.avatar),
            background: create_offset!(content.background),
        };

        let kind0_offset = Kind0Parsed::create(&mut builder, &kind0_args);
        builder.finish(kind0_offset, None);
        Ok((builder.finished_data().to_vec(), Vec::new()))
    }
}
