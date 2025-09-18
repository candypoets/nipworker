use crate::db::index::NostrDB;
use crate::nostr::Template;
use crate::parsed_event::{ParsedData, ParsedEvent};
use crate::signer::interface::SignerManagerInterface;
use crate::signer::manager::SignerManager;

use crate::types::nostr::Event;
use std::sync::Arc;
use thiserror::Error;

/// Parser-specific error type
#[derive(Debug, Error)]
pub enum ParserError {
    #[error("Invalid event kind: {0}")]
    InvalidKind(u32),

    #[error("Missing required field: {0}")]
    MissingField(String),

    #[error("Invalid format: {0}")]
    InvalidFormat(String),

    #[error("Invalid content format: {0}")]
    InvalidContent(String),

    #[error("Cryptographic error: {0}")]
    Crypto(String),

    #[error("Signer error: {0}")]
    SignerError(#[from] crate::signer::SignerError),

    #[error("Types error: {0}")]
    TypesError(#[from] crate::types::TypesError),

    #[error("Invalid tag format: {0}")]
    InvalidTag(String),

    #[error("Parse error: {0}")]
    Parse(String),

    #[error("Other error: {0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, ParserError>;

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
pub use kind39089::Kind39089Parsed;
pub use kind4::Kind4Parsed;
pub use kind6::Kind6Parsed;
pub use kind7::{Emoji, Kind7Parsed, ReactionType};
pub use kind7374::Kind7374Parsed;
pub use kind7375::Kind7375Parsed;
pub use kind7376::{HistoryTag, Kind7376Parsed};
pub use kind9321::Kind9321Parsed;
pub use kind9735::{Kind9735Parsed, ZapRequest};

pub struct Parser {
    pub signer_manager: Arc<SignerManager>,
    pub database: Arc<NostrDB>,
}

impl Parser {
    pub fn new_with_signer(signer_manager: Arc<SignerManager>, database: Arc<NostrDB>) -> Self {
        Self {
            signer_manager,
            database,
        }
    }

    pub fn parse(&self, event: Event) -> Result<ParsedEvent> {
        let kind = event.kind;

        let (parsed, requests) = match kind {
            0 => {
                let (parsed, requests) = self.parse_kind_0(&event)?;
                (Some(ParsedData::Kind0(parsed)), requests)
            }
            1 => {
                let (parsed, requests) = self.parse_kind_1(&event)?;
                (Some(ParsedData::Kind1(parsed)), requests)
            }
            3 => {
                let (parsed, requests) = self.parse_kind_3(&event)?;
                (Some(ParsedData::Kind3(parsed)), requests)
            }
            4 => {
                let (parsed, requests) = self.parse_kind_4(&event)?;
                (Some(ParsedData::Kind4(parsed)), requests)
            }
            6 => {
                let (parsed, requests) = self.parse_kind_6(&event)?;
                (Some(ParsedData::Kind6(parsed)), requests)
            }
            7 => {
                let (parsed, requests) = self.parse_kind_7(&event)?;
                (Some(ParsedData::Kind7(parsed)), requests)
            }
            17 => {
                let (parsed, requests) = self.parse_kind_17(&event)?;
                (Some(ParsedData::Kind17(parsed)), requests)
            }
            7374 => {
                let (parsed, requests) = self.parse_kind_7374(&event)?;
                (Some(ParsedData::Kind7374(parsed)), requests)
            }
            7375 => {
                let (parsed, requests) = self.parse_kind_7375(&event)?;
                (Some(ParsedData::Kind7375(parsed)), requests)
            }
            7376 => {
                let (parsed, requests) = self.parse_kind_7376(&event)?;
                (Some(ParsedData::Kind7376(parsed)), requests)
            }
            9321 => {
                let (parsed, requests) = self.parse_kind_9321(&event)?;
                (Some(ParsedData::Kind9321(parsed)), requests)
            }
            9735 => {
                let (parsed, requests) = self.parse_kind_9735(&event)?;
                (Some(ParsedData::Kind9735(parsed)), requests)
            }
            10002 => {
                let (parsed, requests) = self.parse_kind_10002(&event)?;
                (Some(ParsedData::Kind10002(parsed)), requests)
            }
            10019 => {
                let (parsed, requests) = self.parse_kind_10019(&event)?;
                (Some(ParsedData::Kind10019(parsed)), requests)
            }
            17375 => {
                let (parsed, requests) = self.parse_kind_17375(&event)?;
                (Some(ParsedData::Kind17375(parsed)), requests)
            }
            39089 => {
                let (parsed, requests) = self.parse_kind_39089(&event)?;
                (Some(ParsedData::Kind39089(parsed)), requests)
            }
            _ => {
                return Err(ParserError::InvalidKind(kind as u32));
            }
        };

        Ok(ParsedEvent {
            event,
            parsed,
            requests,
            relays: Vec::new(),
        })
    }

    pub fn prepare(&self, template: &Template) -> Result<Event> {
        let kind = template.kind;

        match kind {
            4 => self.prepare_kind_4(template),
            7374 => self.prepare_kind_7374(template),
            7375 => self.prepare_kind_7375(template),
            7376 => self.prepare_kind_7376(template),
            9321 => self.prepare_kind_9321(template),
            10019 => self.prepare_kind_10019(template),
            17375 => self.prepare_kind_17375(template),
            _ => {
                // Event is already signed - no additional preparation needed
                let new_event = self.signer_manager.sign_event(template)?;
                Ok(new_event)
            }
        }
    }
}
