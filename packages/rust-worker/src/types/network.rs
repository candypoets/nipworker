use std::collections::HashMap;

use nostr::{Alphabet, EventId, Filter, Kind, PublicKey, SingleLetterTag, Timestamp};
use serde::{Deserialize, Serialize};
use tracing::debug;
use wasm_bindgen::prelude::*;

/// Request represents a subscription request
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Request {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub ids: Vec<String>,

    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub authors: Vec<String>,

    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub kinds: Vec<i32>,

    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub tags: HashMap<String, Vec<String>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<i64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<i64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i32>,

    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub search: String,

    pub relays: Vec<String>,

    #[serde(rename = "closeOnEOSE", default)]
    pub close_on_eose: bool,

    #[serde(rename = "cacheFirst", default)]
    pub cache_first: bool,

    #[serde(rename = "noOptimize", default)]
    pub no_optimize: bool,

    #[serde(default)]
    pub count: bool,

    #[serde(rename = "noContext", default)]
    pub no_context: bool,
}

impl Request {
    pub fn new(relays: Vec<String>, filter: Filter) -> Self {
        Self {
            ids: filter
                .ids
                .map(|ids| ids.into_iter().map(|id| id.to_hex()).collect())
                .unwrap_or_default(),
            authors: filter
                .authors
                .map(|authors| authors.into_iter().map(|pk| pk.to_hex()).collect())
                .unwrap_or_default(),
            kinds: filter
                .kinds
                .map(|kinds| kinds.into_iter().map(|k| k.as_u64() as i32).collect())
                .unwrap_or_default(),
            tags: HashMap::new(), // TODO: Convert filter tags properly
            since: filter.since.map(|ts| ts.as_u64() as i64),
            until: filter.until.map(|ts| ts.as_u64() as i64),
            limit: filter.limit.map(|l| l as i32),
            search: filter.search.unwrap_or_default(),
            relays,
            close_on_eose: false,
            cache_first: false,
            no_optimize: false,
            count: false,
            no_context: false,
        }
    }

    pub fn to_filter(&self) -> Result<Filter, anyhow::Error> {
        let mut filter = Filter::new();

        if !self.ids.is_empty() {
            let ids: Result<Vec<EventId>, _> =
                self.ids.iter().map(|id| EventId::from_hex(id)).collect();
            filter = filter.ids(ids?);
        }

        if !self.authors.is_empty() {
            let authors: Result<Vec<PublicKey>, _> = self
                .authors
                .iter()
                .map(|pk| PublicKey::from_hex(pk))
                .collect();
            filter = filter.authors(authors?);
        }

        if !self.kinds.is_empty() {
            let kinds: Vec<Kind> = self.kinds.iter().map(|k| Kind::from(*k as u64)).collect();
            filter = filter.kinds(kinds);
        }

        if let Some(since) = self.since {
            filter = filter.since(Timestamp::from(since as u64));
        }

        if let Some(until) = self.until {
            filter = filter.until(Timestamp::from(until as u64));
        }

        if let Some(limit) = self.limit {
            filter = filter.limit(limit as usize);
        }

        if !self.search.is_empty() {
            filter = filter.search(&self.search);
        }

        // Handle generic tags
        for (tag_name, tag_values) in &self.tags {
            // Tags in Nostr filters are prefixed with '#', so we check for length 2 and extract the second character
            if tag_name.len() == 2 && tag_name.starts_with('#') && !tag_values.is_empty() {
                // Get the second character (the actual tag identifier)
                if let Some(tag_char) = tag_name.chars().nth(1) {
                    match tag_char {
                        'a'..='z' | 'A'..='Z' => {
                            let alphabet = match tag_char.to_ascii_lowercase() {
                                'a' => Some(Alphabet::A),
                                'b' => Some(Alphabet::B),
                                'c' => Some(Alphabet::C),
                                'd' => Some(Alphabet::D),
                                'e' => Some(Alphabet::E),
                                'f' => Some(Alphabet::F),
                                'g' => Some(Alphabet::G),
                                'h' => Some(Alphabet::H),
                                'i' => Some(Alphabet::I),
                                'j' => Some(Alphabet::J),
                                'k' => Some(Alphabet::K),
                                'l' => Some(Alphabet::L),
                                'm' => Some(Alphabet::M),
                                'n' => Some(Alphabet::N),
                                'o' => Some(Alphabet::O),
                                'p' => Some(Alphabet::P),
                                'q' => Some(Alphabet::Q),
                                'r' => Some(Alphabet::R),
                                's' => Some(Alphabet::S),
                                't' => Some(Alphabet::T),
                                'u' => Some(Alphabet::U),
                                'v' => Some(Alphabet::V),
                                'w' => Some(Alphabet::W),
                                'x' => Some(Alphabet::X),
                                'y' => Some(Alphabet::Y),
                                'z' => Some(Alphabet::Z),
                                _ => {
                                    debug!(
                                        "Unexpected character after to_ascii_lowercase: '{}' (original: '{}')",
                                        tag_char.to_ascii_lowercase(),
                                        tag_char
                                    );
                                    None
                                }
                            };

                            if let Some(alphabet) = alphabet {
                                let single_letter_tag = SingleLetterTag::lowercase(alphabet);
                                filter = filter.custom_tag(single_letter_tag, tag_values.clone());
                            }
                        }
                        _ => {
                            // This case handles non-alphabetic characters in tag names
                            // For debugging purposes, let's log the unexpected character
                            debug!("Ignoring non-alphabetic tag name character: '{}'", tag_char);
                            // We could implement special handling for numeric or symbolic tags here if needed
                        }
                    }
                }
            }
        }

        Ok(filter)
    }
}

/// Subscription event types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SubscribeKind {
    #[serde(rename = "CACHED_EVENT")]
    CachedEvent,
    #[serde(rename = "FETCHED_EVENT")]
    FetchedEvent,
    #[serde(rename = "COUNT")]
    Count,
    #[serde(rename = "EOSE")]
    Eose,
    #[serde(rename = "EOCE")]
    Eoce,
}

/// Publish event types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PublishKind {
    #[serde(rename = "PUBLISH_STATUS")]
    PublishStatus,
}

/// Subscription options
#[derive(Debug, Clone)]
pub struct SubscriptionOptions {
    pub close_on_eose: bool,
    pub skip_cache: bool,
    pub force: bool,
}

impl Default for SubscriptionOptions {
    fn default() -> Self {
        Self {
            close_on_eose: false,
            skip_cache: false,
            force: false,
        }
    }
}

/// EOSE (End of Stored Events) represents the completion of stored events delivery
/// This matches the Go type from types/eose.go
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[wasm_bindgen]
pub struct EOSE {
    #[serde(rename = "totalConnections")]
    #[wasm_bindgen(getter_with_clone)]
    pub total_connections: i32,

    #[serde(rename = "remainingConnections")]
    #[wasm_bindgen(getter_with_clone)]
    pub remaining_connections: i32,
}

#[wasm_bindgen]
impl EOSE {
    #[wasm_bindgen(constructor)]
    pub fn new(total_connections: i32, remaining_connections: i32) -> Self {
        Self {
            total_connections,
            remaining_connections,
        }
    }

    /// Check if all connections are complete (remaining connections is 0)
    #[wasm_bindgen]
    pub fn is_complete(&self) -> bool {
        self.remaining_connections == 0
    }

    /// Get the number of completed connections
    #[wasm_bindgen]
    pub fn completed_connections(&self) -> i32 {
        self.total_connections - self.remaining_connections
    }

    /// Get the completion percentage (0.0 to 1.0)
    #[wasm_bindgen]
    pub fn completion_percentage(&self) -> f64 {
        if self.total_connections == 0 {
            1.0
        } else {
            (self.total_connections - self.remaining_connections) as f64
                / self.total_connections as f64
        }
    }

    /// Convert to JSON string
    #[wasm_bindgen]
    pub fn to_json(&self) -> Result<String, JsValue> {
        serde_json::to_string(self).map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Create from JSON string
    #[wasm_bindgen]
    pub fn from_json(json: &str) -> Result<EOSE, JsValue> {
        serde_json::from_str(json).map_err(|e| JsValue::from_str(&e.to_string()))
    }
}

impl Default for EOSE {
    fn default() -> Self {
        Self {
            total_connections: 0,
            remaining_connections: 0,
        }
    }
}

impl std::fmt::Display for EOSE {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "EOSE(total: {}, remaining: {}, completed: {})",
            self.total_connections,
            self.remaining_connections,
            self.completed_connections()
        )
    }
}
