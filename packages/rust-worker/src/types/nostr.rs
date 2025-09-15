use anyhow::{anyhow, Result};
use k256::schnorr::{Signature, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

// Import the signature traits we need
use k256::schnorr::signature::{Signer, Verifier};

// Just re-export what we need from k256
pub const SECP256K1: k256::Secp256k1 = k256::Secp256k1;

// ============================================================================
// Basic Types - Just byte arrays
// ============================================================================
#[derive(Deserialize, Clone, Copy)]
pub struct EventId(pub [u8; 32]);

impl EventId {
    pub fn from_hex(s: &str) -> Result<Self> {
        let bytes = hex::decode(s)?;
        if bytes.len() != 32 {
            return Err(anyhow!("Invalid event ID"));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(EventId(arr))
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    pub fn to_bytes(&self) -> [u8; 32] {
        self.0
    }

    pub fn to_string(&self) -> String {
        self.to_hex()
    }
}

#[derive(Deserialize, Clone)]
pub struct PublicKey(pub [u8; 32]);

impl PublicKey {
    pub fn from_hex(s: &str) -> Result<Self> {
        let bytes = hex::decode(s)?;
        if bytes.len() != 32 {
            return Err(anyhow!("Invalid pubkey"));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(PublicKey(arr))
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    pub fn to_string(&self) -> String {
        self.to_hex()
    }
}

pub struct SecretKey(pub [u8; 32]);

impl SecretKey {
    pub fn from_hex(s: &str) -> Result<Self> {
        let bytes = hex::decode(s)?;
        if bytes.len() != 32 {
            return Err(anyhow!("Invalid secret key"));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(SecretKey(arr))
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    pub fn public_key(&self, _secp: &k256::Secp256k1) -> PublicKey {
        let signing_key = SigningKey::from_bytes(&self.0).unwrap();
        let verifying_key = signing_key.verifying_key();
        PublicKey(verifying_key.to_bytes().into())
    }

    pub fn display_secret(&self) -> String {
        self.to_hex()
    }
}

pub struct Keys {
    pub secret_key: SecretKey,
    pub public_key: PublicKey,
}

impl Keys {
    pub fn new(secret_key: SecretKey) -> Self {
        let public_key = secret_key.public_key(&SECP256K1);
        Self {
            secret_key,
            public_key,
        }
    }

    pub fn parse(nsec: &str) -> Result<Self> {
        // Check if it starts with "nsec1" for bech32 format
        if nsec.starts_with("nsec1") {
            // For now, return error since bech32 is not implemented
            return Err(anyhow!("Bech32 nsec parsing not implemented"));
        }

        // Otherwise treat it as hex
        let secret_key = SecretKey::from_hex(nsec)?;
        Ok(Self::new(secret_key))
    }

    pub fn generate() -> Self {
        let signing_key = SigningKey::random(&mut k256::elliptic_curve::rand_core::OsRng);
        let secret_bytes: [u8; 32] = signing_key.to_bytes().into();
        Self::new(SecretKey(secret_bytes))
    }

    pub fn secret_key(&self) -> Result<&SecretKey> {
        Ok(&self.secret_key)
    }

    pub fn public_key(&self) -> PublicKey {
        PublicKey(self.public_key.0)
    }
}

// ============================================================================
// Event & Filter
// ============================================================================

// Just use u64 directly for timestamp
pub type Timestamp = u64;

#[derive(Serialize, Deserialize)]
pub struct Template {
    pub kind: Kind,
    pub content: String,
    pub tags: Vec<Vec<String>>,
}

impl Template {
    pub fn new(kind: Kind, content: String, tags: Vec<Vec<String>>) -> Self {
        Template {
            kind,
            content,
            tags,
        }
    }

    pub fn to_event(&self, keys: &Keys) -> Result<Event> {
        let created_at = timestamp_now();
        let pubkey = keys.public_key();

        let mut event = Event {
            id: EventId([0u8; 32]), // Will be computed
            pubkey,
            created_at,
            kind: self.kind,
            tags: self.tags.clone(),
            content: self.content.clone(),
            sig: String::new(), // Will be computed
        };

        // Compute the event ID
        event.compute_id()?;

        // Sign the event
        let signing_key = SigningKey::from_bytes(&keys.secret_key.0)?;
        let signature = signing_key.sign(&event.id.0);
        event.sig = hex::encode(signature.to_bytes());

        Ok(event)
    }
}

pub struct UnsignedEvent {
    pub pubkey: PublicKey,
    pub kind: Kind,
    pub content: String,
    pub tags: Vec<Vec<String>>,
}

impl UnsignedEvent {
    pub fn new(pubkey: &str, kind: Kind, content: String, tags: Vec<Vec<String>>) -> Result<Self> {
        let pubkey = PublicKey::from_hex(pubkey)?;
        Ok(UnsignedEvent {
            pubkey,
            kind,
            content,
            tags,
        })
    }
}

// Single struct for both signed and unsigned events

#[derive(Deserialize, Clone)]
pub struct Event {
    pub id: EventId,
    pub pubkey: PublicKey,
    pub created_at: Timestamp,
    pub kind: Kind,
    pub tags: Vec<Vec<String>>,
    pub content: String,
    pub sig: String,
}

impl Event {
    /// Getter for kind
    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// Getter for tags
    pub fn tags(&self) -> &Vec<Vec<String>> {
        &self.tags
    }

    /// Getter for content
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Compute the event ID
    pub fn compute_id(&mut self) -> Result<()> {
        let tags_json = serde_json::to_string(&self.tags)?;
        let serialized = format!(
            "[0,\"{}\",{},{},{},\"{}\"]",
            self.pubkey.to_hex(),
            self.created_at,
            self.kind,
            tags_json,
            self.content.replace('\\', "\\\\").replace('"', "\\\"")
        );

        let mut hasher = Sha256::new();
        hasher.update(serialized.as_bytes());
        let result = hasher.finalize();
        let mut id_bytes = [0u8; 32];
        id_bytes.copy_from_slice(&result);
        self.id = EventId(id_bytes);
        Ok(())
    }

    /// Verify signature
    pub fn verify(&self) -> Result<()> {
        // Check that all required fields are present
        if self.id.0 == [0u8; 32] {
            return Err(anyhow!("No ID"));
        }
        if self.pubkey.0 == [0u8; 32] {
            return Err(anyhow!("No pubkey"));
        }
        if self.sig.is_empty() {
            return Err(anyhow!("No signature"));
        }

        // Verify ID matches
        let mut temp = Event {
            id: EventId([0u8; 32]),
            pubkey: PublicKey(self.pubkey.0),
            created_at: self.created_at,
            kind: self.kind,
            tags: self.tags.clone(),
            content: self.content.clone(),
            sig: String::new(),
        };
        temp.compute_id()?;

        if temp.id.0 != self.id.0 {
            return Err(anyhow!("ID mismatch"));
        }

        // Verify signature
        let verifying_key = VerifyingKey::from_bytes(&self.pubkey.0)?;
        let signature = Signature::try_from(hex::decode(&self.sig)?.as_slice())?;
        verifying_key.verify(&self.id.0, &signature)?;

        Ok(())
    }

    pub fn as_json(&self) -> String {
        // Manual JSON serialization to avoid serde overhead
        let tags_json = serde_json::to_string(&self.tags).unwrap_or_default();
        format!(
            r#"{{"id":"{}","pubkey":"{}","created_at":{},"kind":{},"tags":{},"content":"{}","sig":"{}"}}"#,
            self.id.to_hex(),
            self.pubkey.to_hex(),
            self.created_at,
            self.kind,
            tags_json,
            self.content.replace('\\', "\\\\").replace('"', "\\\""),
            self.sig
        )
    }

    pub fn from_json(json: &str) -> Result<Self> {
        let v: serde_json::Value = serde_json::from_str(json)?;

        let id = v["id"]
            .as_str()
            .and_then(|s| EventId::from_hex(s).ok())
            .unwrap_or(EventId([0u8; 32]));

        let pubkey = v["pubkey"]
            .as_str()
            .and_then(|s| PublicKey::from_hex(s).ok())
            .unwrap_or(PublicKey([0u8; 32]));

        let sig = v["sig"].as_str().map(|s| s.to_string()).unwrap_or_default();

        Ok(Event {
            id,
            pubkey,
            created_at: v["created_at"].as_u64().unwrap_or(0),
            kind: v["kind"].as_u64().unwrap_or(0) as Kind,
            tags: serde_json::from_value(v["tags"].clone()).unwrap_or_default(),
            content: v["content"].as_str().unwrap_or("").to_string(),
            sig,
        })
    }
}

// Minimal filter for queries
#[derive(Clone)]
pub struct Filter {
    pub ids: Option<Vec<EventId>>,
    pub authors: Option<Vec<PublicKey>>,
    pub kinds: Option<Vec<Kind>>,
    pub e_tags: Option<Vec<String>>,
    pub p_tags: Option<Vec<String>>,
    pub d_tags: Option<Vec<String>>,
    pub a_tags: Option<Vec<String>>,
    pub since: Option<Timestamp>,
    pub until: Option<Timestamp>,
    pub limit: Option<usize>,
    pub search: Option<String>,
}

impl Filter {
    pub fn new() -> Self {
        Filter {
            ids: None,
            authors: None,
            kinds: None,
            e_tags: None,
            p_tags: None,
            d_tags: None,
            a_tags: None,
            since: None,
            until: None,
            limit: None,
            search: None,
        }
    }

    pub fn id(mut self, id: EventId) -> Self {
        self.ids.get_or_insert(vec![]).push(id);
        self
    }

    pub fn author(mut self, author: PublicKey) -> Self {
        self.authors.get_or_insert(vec![]).push(author);
        self
    }

    pub fn kind(mut self, kind: Kind) -> Self {
        self.kinds.get_or_insert(vec![]).push(kind);
        self
    }

    pub fn custom_tag(mut self, tag_name: &str, values: Vec<String>) -> Self {
        match tag_name {
            "e" => self.e_tags = Some(values),
            "p" => self.p_tags = Some(values),
            "d" => self.d_tags = Some(values),
            "a" => self.a_tags = Some(values),
            _ => {} // Ignore others for now
        }
        self
    }

    pub fn as_json(&self) -> String {
        let mut json = "{".to_string();

        if let Some(ref ids) = self.ids {
            json.push_str(r#""ids":["#);
            json.push_str(
                &ids.iter()
                    .map(|id| format!(r#""{}""#, id.to_hex()))
                    .collect::<Vec<_>>()
                    .join(","),
            );
            json.push_str("],");
        }

        if let Some(ref authors) = self.authors {
            json.push_str(r#""authors":["#);
            json.push_str(
                &authors
                    .iter()
                    .map(|author| format!(r#""{}""#, author.to_hex()))
                    .collect::<Vec<_>>()
                    .join(","),
            );
            json.push_str("],");
        }

        if let Some(ref kinds) = self.kinds {
            json.push_str(r#""kinds":["#);
            json.push_str(
                &kinds
                    .iter()
                    .map(|k| k.to_string())
                    .collect::<Vec<_>>()
                    .join(","),
            );
            json.push_str("],");
        }

        // if let Some(ref e_tags) = self.e_tags {
        //     json.push_str(r#""#e":["#);
        //     json.push_str(&e_tags.iter().map(|s| format!(r#""{}""#, s.replace('\\', "\\\\").replace('"', "\\\""))).collect::<Vec<_>>().join(","));
        //     json.push_str("],");
        // }

        // if let Some(ref p_tags) = self.p_tags {
        //     json.push_str(r#""#p":["#);
        //     json.push_str(&p_tags.iter().map(|s| format!(r#""{}""#, s.replace('\\', "\\\\").replace('"', "\\\""))).collect::<Vec<_>>().join(","));
        //     json.push_str("],");
        // }

        // if let Some(ref d_tags) = self.d_tags {
        //     json.push_str(r#""#d":["#);
        //     json.push_str(&d_tags.iter().map(|s| format!(r#""{}""#, s.replace('\\', "\\\\").replace('"', "\\\""))).collect::<Vec<_>>().join(","));
        //     json.push_str("],");
        // }

        // if let Some(ref a_tags) = self.a_tags {
        //     json.push_str(r#""#a":["#);
        //     json.push_str(&a_tags.iter().map(|s| format!(r#""{}""#, s.replace('\\', "\\\\").replace('"', "\\\""))).collect::<Vec<_>>().join(","));
        //     json.push_str("],");
        // }

        if let Some(since) = self.since {
            json.push_str(&format!(r#""since":{},"#, since));
        }

        if let Some(until) = self.until {
            json.push_str(&format!(r#""until":{},"#, until));
        }

        if let Some(limit) = self.limit {
            json.push_str(&format!(r#""limit":{},"#, limit));
        }

        if let Some(ref search) = self.search {
            json.push_str(&format!(
                r#""search":"{}""#,
                search.replace('\\', "\\\\").replace('"', "\\\"")
            ));
        }

        // Remove trailing comma if present
        if json.ends_with(',') {
            json.pop();
        }

        json.push('}');
        json
    }
}

// Kind constants - just functions, not impl on u64
pub type Kind = u16;
pub const METADATA: Kind = 0;
pub const TEXT_NOTE: Kind = 1;
pub const CONTACT_LIST: Kind = 3;
pub const ENCRYPTED_DIRECT_MESSAGE: Kind = 4;
pub const DELETION: Kind = 5;
pub const REPOST: Kind = 6;
pub const REACTION: Kind = 7;
pub const RELAY_LIST: Kind = 10002;

// Timestamp helper
pub fn timestamp_now() -> Timestamp {
    (js_sys::Date::now() / 1000.0) as u64
}

// ============================================================================
// NIP-19 (Bech32) - Stubbed out since bech32 is not in dependencies
// ============================================================================

pub mod nips {
    pub mod nip19 {
        use super::super::*;

        pub enum Nip19 {
            Pubkey(PublicKey),
            EventId(EventId),
            Profile(Nip19Profile),
            Event(Nip19Event),
        }

        pub struct Nip19Profile {
            pub public_key: PublicKey,
            pub relays: Vec<String>,
        }

        pub struct Nip19Event {
            pub event_id: EventId,
            pub author: Option<PublicKey>,
            pub relays: Vec<String>,
        }

        pub trait FromBech32 {
            fn from_bech32(s: &str) -> Result<Self>
            where
                Self: Sized;
        }

        impl FromBech32 for Nip19 {
            fn from_bech32(_s: &str) -> Result<Self> {
                // Stub implementation - add bech32 crate if needed
                Err(anyhow!("Bech32 decoding not implemented"))
            }
        }
    }

    // NIP-04 encryption if needed
    pub mod nip04 {
        use super::super::*;

        pub fn encrypt(
            _secret_key: &SecretKey,
            _pubkey: &PublicKey,
            _text: &str,
        ) -> Result<String> {
            // Implement only if you actually use it
            Ok(String::new())
        }

        pub fn decrypt(
            _secret_key: &SecretKey,
            _pubkey: &PublicKey,
            _text: &str,
        ) -> Result<String> {
            // Implement only if you actually use it
            Ok(String::new())
        }
    }

    // NIP-44 encryption if needed
    pub mod nip44 {
        use super::super::*;

        pub fn encrypt(
            _secret_key: &SecretKey,
            _pubkey: &PublicKey,
            _text: &str,
        ) -> Result<String> {
            // Implement only if you actually use it
            Ok(String::new())
        }

        pub fn decrypt(
            _secret_key: &SecretKey,
            _pubkey: &PublicKey,
            _text: &str,
        ) -> Result<String> {
            // Implement only if you actually use it
            Ok(String::new())
        }
    }
}

// Re-export for compatibility
pub use nips::*;
