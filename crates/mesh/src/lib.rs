//! Transport-neutral Nostr mesh sessions.
//!
//! Platform adapters only have to deliver complete UTF-8 Nostr frames to a
//! directly connected peer. Discovery, GATT, MTU negotiation, permissions and
//! background execution deliberately live outside this crate.

use std::collections::HashMap;

use negentropy::{Id, Negentropy, NegentropyStorageVector};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thiserror::Error;

pub mod cache;
pub mod framing;
pub mod runtime;
pub mod session;
pub mod transport;

const NEGENTROPY_FRAME_SIZE_LIMIT: u64 = 4096;

#[derive(Debug, Error)]
pub enum MeshError {
    #[error("invalid event id: {0}")]
    InvalidEventId(String),
    #[error("invalid Nostr event: {0}")]
    InvalidEvent(String),
    #[error("invalid Nostr frame")]
    InvalidFrame,
    #[error("negentropy error: {0}")]
    Negentropy(#[from] negentropy::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("mesh cache channel closed")]
    CacheChannelClosed,
    #[error("invalid mesh cache response")]
    InvalidCacheResponse,
    #[error("mesh cache returned a non-NostrEvent record")]
    InvalidCacheRecord,
    #[error("unexpected mesh cache response: expected {expected}, got {actual}")]
    UnexpectedCacheResponse { expected: String, actual: String },
    #[error("unknown NIP-77 session: {0}")]
    UnknownSession(String),
    #[error("unexpected NIP-77 message for session: {0}")]
    UnexpectedSessionMessage(String),
    #[error("BLE frame error: {0}")]
    Frame(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalEvent {
    pub id: String,
    pub pubkey: String,
    pub created_at: u64,
    pub kind: u16,
    pub tags: Vec<Vec<String>>,
    pub content: String,
    pub sig: String,
}

impl CanonicalEvent {
    fn negentropy_id(&self) -> Result<Id, MeshError> {
        let bytes =
            hex::decode(&self.id).map_err(|_| MeshError::InvalidEventId(self.id.clone()))?;
        Id::from_slice(&bytes).map_err(MeshError::from)
    }
}

/// Minimal event-store contract required by the session engine.
///
/// The production adapter will implement this over the mesh CacheWorker
/// channel, whose records are `WorkerMessage<NostrEvent>`.
pub trait MeshEventStore {
    fn events(&self) -> Vec<CanonicalEvent>;
    fn event(&self, id: &str) -> Option<CanonicalEvent>;
    /// Returns true only when the event ID was absent before this call.
    fn persist(&mut self, event: CanonicalEvent) -> bool;
}

#[derive(Default)]
pub struct MemoryEventStore {
    events: HashMap<String, CanonicalEvent>,
}

impl MeshEventStore for MemoryEventStore {
    fn events(&self) -> Vec<CanonicalEvent> {
        self.events.values().cloned().collect()
    }

    fn event(&self, id: &str) -> Option<CanonicalEvent> {
        self.events.get(id).cloned()
    }

    fn persist(&mut self, event: CanonicalEvent) -> bool {
        if self.events.contains_key(&event.id) {
            return false;
        }
        self.events.insert(event.id.clone(), event);
        true
    }
}

fn negentropy_storage(store: &impl MeshEventStore) -> Result<NegentropyStorageVector, MeshError> {
    let events = store.events();
    let mut storage = NegentropyStorageVector::with_capacity(events.len());
    for event in events {
        storage.insert(event.created_at, event.negentropy_id()?)?;
    }
    storage.seal()?;
    Ok(storage)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NostrFrame(String);

impl NostrFrame {
    pub fn from_string(frame: String) -> Result<Self, MeshError> {
        let value: Value = serde_json::from_str(&frame)?;
        if !value.is_array() {
            return Err(MeshError::InvalidFrame);
        }
        Ok(Self(frame))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn neg_open(subscription_id: &str, filter: &Value, payload: &[u8]) -> Self {
        Self(json!(["NEG-OPEN", subscription_id, filter, hex::encode(payload)]).to_string())
    }

    pub(crate) fn neg_msg(subscription_id: &str, payload: &[u8]) -> Self {
        Self(json!(["NEG-MSG", subscription_id, hex::encode(payload)]).to_string())
    }

    pub(crate) fn neg_close(subscription_id: &str) -> Self {
        Self(json!(["NEG-CLOSE", subscription_id]).to_string())
    }

    pub(crate) fn request(subscription_id: &str, ids: &[String]) -> Self {
        Self(json!(["REQ", subscription_id, { "ids": ids }]).to_string())
    }

    pub(crate) fn publish(event: &CanonicalEvent) -> Self {
        Self(json!(["EVENT", event]).to_string())
    }

    pub(crate) fn response(subscription_id: &str, event: &CanonicalEvent) -> Self {
        Self(json!(["EVENT", subscription_id, event]).to_string())
    }

    pub(crate) fn eose(subscription_id: &str) -> Self {
        Self(json!(["EOSE", subscription_id]).to_string())
    }
}

/// In-memory stand-in for the future FFI-backed BLE byte transport.
#[derive(Default)]
pub struct MemoryLink {
    a_to_b: Vec<NostrFrame>,
    b_to_a: Vec<NostrFrame>,
}

impl MemoryLink {
    pub fn a_to_b(&self) -> &[NostrFrame] {
        &self.a_to_b
    }

    pub fn b_to_a(&self) -> &[NostrFrame] {
        &self.b_to_a
    }

    fn send_a_to_b(&mut self, frame: NostrFrame) {
        self.a_to_b.push(frame);
    }

    fn send_b_to_a(&mut self, frame: NostrFrame) {
        self.b_to_a.push(frame);
    }

    pub fn clear(&mut self) {
        self.a_to_b.clear();
        self.b_to_a.clear();
    }
}

pub struct MeshNode<S> {
    pub store: S,
}

impl<S> MeshNode<S>
where
    S: MeshEventStore,
{
    pub fn new(store: S) -> Self {
        Self { store }
    }

    /// Run one complete NIP-77 reconciliation round over a direct-neighbor
    /// link, then exchange the missing events with standard NIP-01 frames.
    pub fn reconcile_with<T>(
        &mut self,
        remote: &mut MeshNode<T>,
        link: &mut MemoryLink,
        subscription_id: &str,
        filter: Value,
    ) -> Result<ReconciliationResult, MeshError>
    where
        T: MeshEventStore,
    {
        let local_storage = negentropy_storage(&self.store)?;
        let remote_storage = negentropy_storage(&remote.store)?;
        let mut initiator = Negentropy::owned(local_storage, NEGENTROPY_FRAME_SIZE_LIMIT)?;
        let mut responder = Negentropy::owned(remote_storage, NEGENTROPY_FRAME_SIZE_LIMIT)?;

        let initial = initiator.initiate()?;
        link.send_a_to_b(NostrFrame::neg_open(subscription_id, &filter, &initial));

        let mut query = initial;
        let mut have_ids = Vec::new();
        let mut need_ids = Vec::new();
        loop {
            let response = responder.reconcile(&query)?;
            link.send_b_to_a(NostrFrame::neg_msg(subscription_id, &response));
            match initiator.reconcile_with_ids(&response, &mut have_ids, &mut need_ids)? {
                Some(next) => {
                    link.send_a_to_b(NostrFrame::neg_msg(subscription_id, &next));
                    query = next;
                }
                None => break,
            }
        }

        let have: Vec<String> = have_ids
            .into_iter()
            .map(|id| hex::encode(id.to_bytes()))
            .collect();
        let need: Vec<String> = need_ids
            .into_iter()
            .map(|id| hex::encode(id.to_bytes()))
            .collect();

        let mut sent_to_remote = 0;
        for id in &have {
            if let Some(event) = self.store.event(id) {
                link.send_a_to_b(NostrFrame::publish(&event));
                if remote.store.persist(event) {
                    sent_to_remote += 1;
                }
            }
        }

        let request_id = format!("{subscription_id}:missing");
        if !need.is_empty() {
            link.send_a_to_b(NostrFrame::request(&request_id, &need));
        }
        let mut received_from_remote = 0;
        for id in &need {
            if let Some(event) = remote.store.event(id) {
                link.send_b_to_a(NostrFrame::response(&request_id, &event));
                if self.store.persist(event) {
                    received_from_remote += 1;
                }
            }
        }
        if !need.is_empty() {
            link.send_b_to_a(NostrFrame::eose(&request_id));
        }
        link.send_a_to_b(NostrFrame::neg_close(subscription_id));

        Ok(ReconciliationResult {
            sent_to_remote,
            received_from_remote,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReconciliationResult {
    pub sent_to_remote: usize,
    pub received_from_remote: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(id_byte: u8, pubkey_byte: u8, created_at: u64) -> CanonicalEvent {
        CanonicalEvent {
            id: hex::encode([id_byte; 32]),
            pubkey: hex::encode([pubkey_byte; 32]),
            created_at,
            kind: 0,
            tags: vec![],
            content: format!(r#"{{"name":"peer-{pubkey_byte}"}}"#),
            sig: hex::encode([0x55; 64]),
        }
    }

    fn numbered_event(number: u64, owner: u8, created_at: u64) -> CanonicalEvent {
        let mut id = [0u8; 32];
        id[24..].copy_from_slice(&number.to_be_bytes());
        CanonicalEvent {
            id: hex::encode(id),
            pubkey: hex::encode([owner; 32]),
            created_at,
            kind: 0,
            tags: vec![],
            content: format!(r#"{{"name":"event-{number}"}}"#),
            sig: hex::encode([0x55; 64]),
        }
    }

    fn frame_kind(frame: &NostrFrame) -> String {
        let value: Value = serde_json::from_str(frame.as_str()).unwrap();
        value[0].as_str().unwrap().to_string()
    }

    fn assert_standard_frame_shapes(link: &MemoryLink) {
        for frame in link.a_to_b().iter().chain(link.b_to_a()) {
            let value: Value = serde_json::from_str(frame.as_str()).unwrap();
            let array = value.as_array().expect("Nostr frame must be a JSON array");
            let kind = array[0].as_str().unwrap();
            match kind {
                "NEG-OPEN" => {
                    assert_eq!(array.len(), 4);
                    assert!(array[1].is_string());
                    assert!(array[2].is_object());
                    assert!(hex::decode(array[3].as_str().unwrap()).is_ok());
                }
                "NEG-MSG" => {
                    assert_eq!(array.len(), 3);
                    assert!(array[1].is_string());
                    assert!(hex::decode(array[2].as_str().unwrap()).is_ok());
                }
                "NEG-CLOSE" | "EOSE" => assert_eq!(array.len(), 2),
                "REQ" => {
                    assert_eq!(array.len(), 3);
                    assert!(array[2]["ids"].is_array());
                }
                "EVENT" => assert!(array.len() == 2 || array.len() == 3),
                other => panic!("unexpected Nostr frame type {other}"),
            }
        }
    }

    #[test]
    fn two_nodes_converge_over_nip77_and_nip01() {
        let event_a = event(0x11, 0xa1, 100);
        let event_b = event(0x22, 0xb2, 200);
        let mut store_a = MemoryEventStore::default();
        let mut store_b = MemoryEventStore::default();
        assert!(store_a.persist(event_a.clone()));
        assert!(store_b.persist(event_b.clone()));

        let mut node_a = MeshNode::new(store_a);
        let mut node_b = MeshNode::new(store_b);
        let mut link = MemoryLink::default();
        let result = node_a
            .reconcile_with(
                &mut node_b,
                &mut link,
                "mesh-kind0",
                json!({ "kinds": [0] }),
            )
            .unwrap();

        assert_eq!(result.sent_to_remote, 1);
        assert_eq!(result.received_from_remote, 1);
        assert!(node_a.store.event(&event_b.id).is_some());
        assert!(node_b.store.event(&event_a.id).is_some());
        assert!(link
            .a_to_b()
            .iter()
            .any(|frame| frame.as_str().starts_with("[\"NEG-OPEN\"")));
        assert!(link
            .a_to_b()
            .iter()
            .any(|frame| frame.as_str().starts_with("[\"EVENT\",")));
        assert!(link
            .b_to_a()
            .iter()
            .any(|frame| frame.as_str().starts_with("[\"EOSE\"")));

        link.clear();
        let second = node_a
            .reconcile_with(
                &mut node_b,
                &mut link,
                "mesh-kind0-2",
                json!({ "kinds": [0] }),
            )
            .unwrap();
        assert_eq!(second, ReconciliationResult::default());
        assert!(!link
            .a_to_b()
            .iter()
            .any(|frame| frame.as_str().starts_with("[\"EVENT\",")));
    }

    #[test]
    fn empty_nodes_converge_without_event_frames() {
        let mut node_a = MeshNode::new(MemoryEventStore::default());
        let mut node_b = MeshNode::new(MemoryEventStore::default());
        let mut link = MemoryLink::default();

        let result = node_a
            .reconcile_with(&mut node_b, &mut link, "empty", json!({ "kinds": [0] }))
            .unwrap();

        assert_eq!(result, ReconciliationResult::default());
        assert!(node_a.store.events().is_empty());
        assert!(node_b.store.events().is_empty());
        assert!(!link
            .a_to_b()
            .iter()
            .chain(link.b_to_a())
            .any(|frame| frame_kind(frame) == "EVENT"));
        assert_standard_frame_shapes(&link);
    }

    #[test]
    fn identical_nodes_do_not_transfer_events() {
        let shared = event(0x33, 0xc3, 300);
        let mut store_a = MemoryEventStore::default();
        let mut store_b = MemoryEventStore::default();
        assert!(store_a.persist(shared.clone()));
        assert!(store_b.persist(shared));
        let mut node_a = MeshNode::new(store_a);
        let mut node_b = MeshNode::new(store_b);
        let mut link = MemoryLink::default();

        let result = node_a
            .reconcile_with(&mut node_b, &mut link, "identical", json!({ "kinds": [0] }))
            .unwrap();

        assert_eq!(result, ReconciliationResult::default());
        assert!(!link
            .a_to_b()
            .iter()
            .chain(link.b_to_a())
            .any(|frame| frame_kind(frame) == "EVENT"));
    }

    #[test]
    fn empty_node_downloads_all_remote_events() {
        let mut remote_store = MemoryEventStore::default();
        for i in 0..10 {
            assert!(remote_store.persist(numbered_event(i, 0xb0, 1_000 + i)));
        }
        let mut node_a = MeshNode::new(MemoryEventStore::default());
        let mut node_b = MeshNode::new(remote_store);
        let mut link = MemoryLink::default();

        let result = node_a
            .reconcile_with(&mut node_b, &mut link, "download", json!({ "kinds": [0] }))
            .unwrap();

        assert_eq!(result.sent_to_remote, 0);
        assert_eq!(result.received_from_remote, 10);
        assert_eq!(node_a.store.events().len(), 10);
        assert_eq!(
            link.b_to_a()
                .iter()
                .filter(|frame| frame_kind(frame) == "EVENT")
                .count(),
            10
        );
        assert_eq!(frame_kind(link.b_to_a().last().unwrap()), "EOSE");
        assert_standard_frame_shapes(&link);
    }

    #[test]
    fn large_disjoint_sets_require_multiple_negentropy_messages_and_converge() {
        let mut store_a = MemoryEventStore::default();
        let mut store_b = MemoryEventStore::default();
        for i in 0..600 {
            assert!(store_a.persist(numbered_event(i, 0xa0, 10_000 + i)));
        }
        for i in 600..1_200 {
            assert!(store_b.persist(numbered_event(i, 0xb0, 10_000 + i)));
        }
        let mut node_a = MeshNode::new(store_a);
        let mut node_b = MeshNode::new(store_b);
        let mut link = MemoryLink::default();

        let result = node_a
            .reconcile_with(&mut node_b, &mut link, "large", json!({ "kinds": [0] }))
            .unwrap();

        assert_eq!(result.sent_to_remote, 600);
        assert_eq!(result.received_from_remote, 600);
        assert_eq!(node_a.store.events().len(), 1_200);
        assert_eq!(node_b.store.events().len(), 1_200);
        assert!(
            link.a_to_b()
                .iter()
                .filter(|frame| frame_kind(frame) == "NEG-MSG")
                .count()
                > 1,
            "the 4096-byte limit should force multiple initiator NEG-MSG frames"
        );

        link.clear();
        let repeat = node_a
            .reconcile_with(
                &mut node_b,
                &mut link,
                "large-repeat",
                json!({ "kinds": [0] }),
            )
            .unwrap();
        assert_eq!(repeat, ReconciliationResult::default());
    }

    #[test]
    fn duplicate_event_id_is_not_persisted_twice() {
        let event = event(0x44, 0xd4, 400);
        let mut store = MemoryEventStore::default();
        assert!(store.persist(event.clone()));
        assert!(!store.persist(event));
        assert_eq!(store.events().len(), 1);
    }

    #[test]
    fn malformed_event_id_fails_before_any_frame_is_sent() {
        let mut bad_store = MemoryEventStore::default();
        assert!(bad_store.persist(CanonicalEvent {
            id: "not-an-event-id".to_string(),
            pubkey: hex::encode([0xaa; 32]),
            created_at: 1,
            kind: 0,
            tags: vec![],
            content: "{}".to_string(),
            sig: hex::encode([0x55; 64]),
        }));
        let mut node_a = MeshNode::new(bad_store);
        let mut node_b = MeshNode::new(MemoryEventStore::default());
        let mut link = MemoryLink::default();

        let error = node_a
            .reconcile_with(&mut node_b, &mut link, "bad", json!({ "kinds": [0] }))
            .unwrap_err();

        assert!(matches!(error, MeshError::InvalidEventId(_)));
        assert!(link.a_to_b().is_empty());
        assert!(link.b_to_a().is_empty());
    }
}
