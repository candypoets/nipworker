use crate::db::index::NostrDB;
use crate::signer::{create_shared_signer_manager, SharedSignerManager};
use crate::types::*;
use anyhow::{anyhow, Result};
use nostr::{Event, Tag, UnsignedEvent};
use std::sync::Arc;
use tracing::info;

// Declare all parser modules
pub mod content;
pub mod kind0;
pub mod kind1;
pub mod kind10002;
pub mod kind10019;
pub mod kind17;
pub mod kind17375;
pub mod kind3;
pub mod kind39089;
pub mod kind4;
pub mod kind6;
pub mod kind7;
pub mod kind7374;
pub mod kind7375;
pub mod kind7376;
pub mod kind9321;
pub mod kind9735;

// Re-export commonly used types
pub use content::{parse_content, ContentBlock, ContentParser};
pub use kind0::{Kind0Parsed, Nip05Response, ProfilePointer};
pub use kind1::{EventPointer, Kind1Parsed, ProfilePointer as Kind1ProfilePointer};
pub use kind10002::{Kind10002Parsed, RelayInfo};
pub use kind10019::{Kind10019Parsed, MintInfo};
pub use kind17::Kind17Parsed;
pub use kind17375::Kind17375Parsed;
pub use kind3::{Contact, Kind3Parsed};
pub use kind39089::Kind30000Parsed;
pub use kind4::Kind4Parsed;
pub use kind6::Kind6Parsed;
pub use kind7::{Emoji, Kind7Parsed, ReactionType};
pub use kind7374::Kind7374Parsed;
pub use kind7375::Kind7375Parsed;
pub use kind7376::{HistoryTag, Kind7376Parsed};
pub use kind9321::Kind9321Parsed;
pub use kind9735::{Kind9735Parsed, ZapRequest};

pub struct Parser {
    pub signer_manager: SharedSignerManager,
    pub database: Arc<NostrDB>,
}

impl Parser {
    pub fn new(database: Arc<NostrDB>) -> Self {
        info!("Creating new parser");
        Self {
            signer_manager: create_shared_signer_manager(),
            database,
        }
    }

    pub fn new_with_signer(signer_manager: SharedSignerManager, database: Arc<NostrDB>) -> Self {
        Self {
            signer_manager,
            database,
        }
    }

    pub fn parse(&self, event: Event) -> Result<ParsedEvent> {
        let kind = event.kind.as_u64();

        let (parsed, requests) = match kind {
            0 => {
                let (parsed, requests) = self.parse_kind_0(&event)?;
                (Some(serde_json::to_value(parsed)?), requests)
            }
            1 => {
                let (parsed, requests) = self.parse_kind_1(&event)?;
                (Some(serde_json::to_value(parsed)?), requests)
            }
            3 => {
                let (parsed, requests) = self.parse_kind_3(&event)?;
                (Some(serde_json::to_value(parsed)?), requests)
            }
            4 => {
                let (parsed, requests) = self.parse_kind_4(&event)?;
                (Some(serde_json::to_value(parsed)?), requests)
            }
            6 => {
                let (parsed, requests) = self.parse_kind_6(&event)?;
                (Some(serde_json::to_value(parsed)?), requests)
            }
            7 => {
                let (parsed, requests) = self.parse_kind_7(&event)?;
                (Some(serde_json::to_value(parsed)?), requests)
            }
            17 => {
                let (parsed, requests) = self.parse_kind_17(&event)?;
                (Some(serde_json::to_value(parsed)?), requests)
            }
            7374 => {
                let (parsed, requests) = self.parse_kind_7374(&event)?;
                (Some(serde_json::to_value(parsed)?), requests)
            }
            7375 => {
                let (parsed, requests) = self.parse_kind_7375(&event)?;
                (Some(serde_json::to_value(parsed)?), requests)
            }
            7376 => {
                let (parsed, requests) = self.parse_kind_7376(&event)?;
                (Some(serde_json::to_value(parsed)?), requests)
            }
            9321 => {
                let (parsed, requests) = self.parse_kind_9321(&event)?;
                (Some(serde_json::to_value(parsed)?), requests)
            }
            9735 => {
                let (parsed, requests) = self.parse_kind_9735(&event)?;
                (Some(serde_json::to_value(parsed)?), requests)
            }
            10002 => {
                let (parsed, requests) = self.parse_kind_10002(&event)?;
                (Some(serde_json::to_value(parsed)?), requests)
            }
            10019 => {
                let (parsed, requests) = self.parse_kind_10019(&event)?;
                (Some(serde_json::to_value(parsed)?), requests)
            }
            17375 => {
                let (parsed, requests) = self.parse_kind_17375(&event)?;
                (Some(serde_json::to_value(parsed)?), requests)
            }
            39089 => {
                let (parsed, requests) = self.parse_kind_39089(&event)?;
                (Some(serde_json::to_value(parsed)?), requests)
            }
            _ => {
                return Err(anyhow!("no parser available for kind {}", kind));
            }
        };

        Ok(ParsedEvent {
            event,
            parsed,
            requests,
            relays: Vec::new(),
        })
    }

    pub fn prepare(&self, event: &mut UnsignedEvent) -> Result<Event> {
        let kind = event.kind.as_u64();

        match kind {
            4 => self.prepare_kind_4(event),
            7374 => self.prepare_kind_7374(event),
            7375 => self.prepare_kind_7375(event),
            7376 => self.prepare_kind_7376(event),
            9321 => self.prepare_kind_9321(event),
            10019 => self.prepare_kind_10019(event),
            17375 => self.prepare_kind_17375(event),
            _ => {
                // Event is already signed - no additional preparation needed
                let new_event = self.signer_manager.sign_event(event)?;
                Ok(new_event)
            }
        }
    }
}

// Helper function to find tag values
pub fn find_tag_value(tags: &[Tag], tag_name: &str) -> Option<String> {
    tags.iter().find_map(|tag| {
        let tag_vec = tag.as_vec();
        if tag_vec.len() >= 2 && tag_vec[0] == tag_name {
            Some(tag_vec[1].clone())
        } else {
            None
        }
    })
}

// Helper function to find all tag values
pub fn find_tag_values(tags: &[Tag], tag_name: &str) -> Vec<String> {
    tags.iter()
        .filter_map(|tag| {
            let tag_vec = tag.as_vec();
            if tag_vec.len() >= 2 && tag_vec[0] == tag_name {
                Some(tag_vec[1].clone())
            } else {
                None
            }
        })
        .collect()
}

// Helper function to find the last tag with a specific name
pub fn find_last_tag<'a>(tags: &'a [Tag], tag_name: &str) -> Option<&'a Tag> {
    tags.iter().rev().find(|tag| {
        let tag_vec = tag.as_vec();
        !tag_vec.is_empty() && tag_vec[0] == tag_name
    })
}

impl Clone for Parser {
    fn clone(&self) -> Self {
        Self {
            signer_manager: self.signer_manager.clone(),
            database: self.database.clone(),
        }
    }
}

impl Default for Parser {
    fn default() -> Self {
        let database = Arc::new(NostrDB::new());
        Self::new(database)
    }
}
