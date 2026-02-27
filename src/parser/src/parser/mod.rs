use shared::types::{nostr::Template, Event, ParserError, TypesError};
use crate::crypto_client::CryptoClient;
use std::cell::RefCell;
use std::sync::Arc;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, ParserError>;

// Declare all parser modules
pub mod content;
pub mod kind0;
pub mod kind1;
pub mod kind10002;
pub mod kind10019;
pub mod kind1111;
pub mod kind1311;
pub mod kind17;
pub mod kind17375;
pub mod kind20;
pub mod kind22;
pub mod kind3;
pub mod kind30023;
pub mod kind4;
pub mod kind6;
pub mod kind7;
pub mod kind7374;
pub mod kind7375;
pub mod kind7376;
pub mod kind9321;
pub mod kind9735;
pub mod nip51;
pub mod pre_adapters;
pub mod pre_generic;

// Re-export commonly used types
pub use content::{parse_content, ContentBlock, ContentParser};
pub use kind0::{Kind0Parsed, Nip05Response, ProfilePointer};
pub use kind1::{EventPointer, Kind1Parsed, ProfilePointer as Kind1ProfilePointer};
pub use kind10002::{Kind10002Parsed, RelayInfo};
pub use kind10019::{Kind10019Parsed, MintInfo};
pub use kind1111::Kind1111Parsed;
pub use kind1311::Kind1311Parsed;
pub use kind17::Kind17Parsed;
pub use kind17375::Kind17375Parsed;
pub use kind20::Kind20Parsed;
pub use kind22::Kind22Parsed;
pub use kind3::{Contact, Kind3Parsed};
pub use kind30023::Kind30023Parsed;
pub use kind4::Kind4Parsed;
pub use kind6::Kind6Parsed;
pub use kind7::{Emoji, Kind7Parsed, ReactionType};
pub use kind7374::Kind7374Parsed;
pub use kind7375::Kind7375Parsed;
pub use kind7376::{HistoryTag, Kind7376Parsed};
pub use kind9321::Kind9321Parsed;
pub use kind9735::{Kind9735Parsed, ZapRequest};
pub use nip51::{Coordinate, ListParsed};
pub use pre_adapters::{
    compute_a_pointer, compute_naddr_like, try_compute_naddr, BadgeDefinition, Calendar,
    CalendarEvent, LiveActivity, LiveSession, LiveSpace, ProfileBadges, WikiArticle, WikiRedirect,
};

use crate::types::parsed_event::{ParsedData, ParsedEvent};

pub struct Parser {
    pub crypto_client: Arc<CryptoClient>,
}

impl Parser {
    pub fn new(crypto_client: Arc<CryptoClient>) -> Self {
        Self { crypto_client }
    }

    pub async fn parse(&self, event: Event) -> Result<ParsedEvent> {
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
                let (parsed, requests) = self.parse_kind_4(&event).await?;
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
            20 => {
                let (parsed, requests) = self.parse_kind_20(&event)?;
                (Some(ParsedData::Kind20(parsed)), requests)
            }
            22 => {
                let (parsed, requests) = self.parse_kind_22(&event)?;
                (Some(ParsedData::Kind22(parsed)), requests)
            }
            1111 => {
                let (parsed, requests) = self.parse_kind_1111(&event)?;
                (Some(ParsedData::Kind1111(parsed)), requests)
            }
            1311 => {
                let (parsed, requests) = self.parse_kind_1311(&event)?;
                (Some(ParsedData::Kind1311(parsed)), requests)
            }
            7374 => {
                let (parsed, requests) = self.parse_kind_7374(&event).await?;
                (Some(ParsedData::Kind7374(parsed)), requests)
            }
            7375 => {
                let (parsed, requests) = self.parse_kind_7375(&event).await?;
                (Some(ParsedData::Kind7375(parsed)), requests)
            }
            7376 => {
                let (parsed, requests) = self.parse_kind_7376(&event).await?;
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
                let (parsed, requests) = self.parse_kind_17375(&event).await?;
                (Some(ParsedData::Kind17375(parsed)), requests)
            }
            30023 => {
                let (parsed, requests) = self.parse_kind_30023(&event)?;
                (Some(ParsedData::Kind30023(parsed)), requests)
            }
            k if (10000..20000).contains(&k) => {
                let (parsed, requests) = self.parse_nip51(&event).await?;
                (Some(ParsedData::List(parsed)), requests)
            }
            k if matches!(
                k,
                30000
                    | 30002
                    | 30003
                    | 30004
                    | 30005
                    | 30007
                    | 30015
                    | 30030
                    | 30063
                    | 30267
                    | 31924
                    | 39089
            ) =>
            {
                let (parsed, requests) = self.parse_nip51(&event).await?;
                (Some(ParsedData::List(parsed)), requests)
            }
            k if (30000..40000).contains(&k) => {
                let (parsed, requests) = self.parse_pre_generic(&event)?;
                (Some(ParsedData::PreGeneric(parsed)), requests)
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

    pub async fn prepare(&self, template: &Template) -> Result<Event> {
        let kind = template.kind;
        let kind_u32 = kind as u32;
        let is_nip51 = (10000..20000).contains(&kind_u32)
            || (30000..40000).contains(&kind_u32)
            || kind_u32 == 39089;

        match kind {
            4 => self.prepare_kind_4(template).await,
            7374 => self.prepare_kind_7374(template).await,
            7375 => self.prepare_kind_7375(template).await,
            7376 => self.prepare_kind_7376(template).await,
            9321 => self.prepare_kind_9321(template).await,
            10019 => self.prepare_kind_10019(template).await,
            9735 => self.prepare_kind_9735(template).await,
            17375 => self.prepare_kind_17375(template).await,
            _ if is_nip51 => self.prepare_nip51(template).await,
            _ => {
                let template_json = template.to_json();
                // Call the async signer client and await the result
                let signed_event_json = self
                    .crypto_client
                    .sign_event(template_json)
                    .await
                    .map_err(|e| ParserError::Crypto(format!("Signer error: {}", e)))?;

                let new_event = Event::from_json(&signed_event_json)?;

                Ok(new_event)
            }
        }
    }
}
