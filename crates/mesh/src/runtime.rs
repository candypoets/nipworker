//! End-to-end Nostr protocol runtime over a directly connected BLE peer.

use std::collections::HashMap;
use std::time::Instant;

use nipworker_core::types::network::Request;
use serde_json::Value;

use crate::cache::MeshCacheClient;
use crate::session::PeerSession;
use crate::transport::{MeshByteTransport, TransportConfig};
use crate::{CanonicalEvent, MemoryEventStore, MeshError, MeshEventStore, NostrFrame};

struct Peer {
    session: PeerSession,
    snapshot: MemoryEventStore,
    next_subscription: u64,
}

pub struct MeshRuntime {
    cache: MeshCacheClient,
    transport: MeshByteTransport,
    peers: HashMap<String, Peer>,
}

impl MeshRuntime {
    pub fn new(cache: MeshCacheClient) -> Self {
        Self {
            cache,
            transport: MeshByteTransport::new(TransportConfig::default()),
            peers: HashMap::new(),
        }
    }

    pub async fn peer_connected(&mut self, peer_id: String, mtu: usize) -> Result<(), MeshError> {
        self.transport.peer_connected(peer_id.clone(), mtu)?;
        let mut snapshot = self.kind_zero_snapshot().await?;
        let mut session = PeerSession::default();
        let subscription_id = format!("mesh-kind0-{}", stable_peer_label(&peer_id));
        let open = session.begin_sync(
            &subscription_id,
            serde_json::json!({ "kinds": [0] }),
            &snapshot,
        )?;
        self.transport.send(&peer_id, &open)?;
        // Keep the cache-derived snapshot for the lifetime of this reconciliation.
        // Incoming events update it immediately, before the next periodic session.
        let _ = &mut snapshot;
        self.peers.insert(
            peer_id,
            Peer {
                session,
                snapshot,
                next_subscription: 0,
            },
        );
        Ok(())
    }

    pub fn peer_disconnected(&mut self, peer_id: &str) {
        self.transport.peer_disconnected(peer_id);
        self.peers.remove(peer_id);
    }

    pub async fn set_local_profile(&self, event: CanonicalEvent) -> Result<(), MeshError> {
        if event.kind != 0 {
            return Err(MeshError::InvalidEvent(
                "mesh profile must be a kind 0 event".to_string(),
            ));
        }
        validate_event(&event)?;
        self.cache.pin_profile(&event).await
    }

    pub async fn clear_local_profile(&self) -> Result<(), MeshError> {
        self.cache.clear_profile().await
    }

    pub fn pop_outbound(&mut self, peer_id: &str) -> Option<Vec<u8>> {
        self.transport.pop_outbound(peer_id)
    }

    pub async fn receive_fragment(
        &mut self,
        peer_id: &str,
        fragment: &[u8],
    ) -> Result<(), MeshError> {
        let Some(frame) = self.transport.receive(peer_id, fragment, Instant::now())? else {
            return Ok(());
        };
        let responses = self.handle_frame(peer_id, &frame).await?;
        for response in responses {
            self.transport.send(peer_id, &response)?;
        }
        Ok(())
    }

    async fn handle_frame(
        &mut self,
        peer_id: &str,
        frame: &NostrFrame,
    ) -> Result<Vec<NostrFrame>, MeshError> {
        let value: Value = serde_json::from_str(frame.as_str())?;
        let array = value.as_array().ok_or(MeshError::InvalidFrame)?;
        let kind = array
            .first()
            .and_then(Value::as_str)
            .ok_or(MeshError::InvalidFrame)?;

        match kind {
            "NEG-OPEN" => {
                let snapshot = self.kind_zero_snapshot().await?;
                let peer = self.peers.get_mut(peer_id).ok_or_else(|| {
                    MeshError::Frame(format!("frame from unknown peer {peer_id}"))
                })?;
                peer.snapshot = snapshot;
                Ok(peer.session.handle(frame, &peer.snapshot)?.frames)
            }
            "NEG-MSG" | "NEG-CLOSE" => {
                let peer = self.peers.get_mut(peer_id).ok_or_else(|| {
                    MeshError::Frame(format!("frame from unknown peer {peer_id}"))
                })?;
                let output = peer.session.handle(frame, &peer.snapshot)?;
                let mut frames = output.frames;
                if let Some(difference) = output.completed {
                    for id in &difference.have {
                        if let Some(event) = peer.snapshot.event(id) {
                            frames.push(NostrFrame::publish(&event));
                        }
                    }
                    if !difference.need.is_empty() {
                        let request_id = format!(
                            "mesh-missing-{}-{}",
                            stable_peer_label(peer_id),
                            peer.next_subscription
                        );
                        peer.next_subscription = peer.next_subscription.wrapping_add(1);
                        frames.push(NostrFrame::request(&request_id, &difference.need));
                    }
                }
                Ok(frames)
            }
            "REQ" => {
                let subscription_id = array
                    .get(1)
                    .and_then(Value::as_str)
                    .ok_or(MeshError::InvalidFrame)?;
                let filters = &array[2..];
                if filters.is_empty() {
                    return Err(MeshError::InvalidFrame);
                }
                let requests = filters
                    .iter()
                    .map(request_from_filter)
                    .collect::<Result<Vec<_>, _>>()?;
                let events = self.cache.query(&requests).await?;
                let mut frames: Vec<_> = events
                    .iter()
                    .map(|event| NostrFrame::response(subscription_id, event))
                    .collect();
                frames.push(NostrFrame::eose(subscription_id));
                Ok(frames)
            }
            "EVENT" => {
                let event_value = match array.len() {
                    2 => array.get(1),
                    3 => array.get(2),
                    _ => None,
                }
                .ok_or(MeshError::InvalidFrame)?;
                let event: CanonicalEvent = serde_json::from_value(event_value.clone())?;
                validate_event(&event)?;
                if event.kind != 0 {
                    return Err(MeshError::InvalidEvent(
                        "initial mesh policy only accepts kind 0".to_string(),
                    ));
                }
                self.cache
                    .persist(&event, &format!("ble://{peer_id}"))
                    .await?;
                if let Some(peer) = self.peers.get_mut(peer_id) {
                    peer.snapshot.persist(event);
                }
                Ok(Vec::new())
            }
            "EOSE" => Ok(Vec::new()),
            _ => Err(MeshError::UnexpectedSessionMessage(kind.to_string())),
        }
    }

    async fn kind_zero_snapshot(&mut self) -> Result<MemoryEventStore, MeshError> {
        let events = self
            .cache
            .query(&[Request {
                kinds: vec![0],
                cache_only: true,
                ..Default::default()
            }])
            .await?;
        let mut snapshot = MemoryEventStore::default();
        for event in events {
            snapshot.persist(event);
        }
        Ok(snapshot)
    }
}

fn request_from_filter(filter: &Value) -> Result<Request, MeshError> {
    let object = filter.as_object().ok_or(MeshError::InvalidFrame)?;
    let strings = |name: &str| -> Result<Vec<String>, MeshError> {
        object
            .get(name)
            .map(|value| {
                value
                    .as_array()
                    .ok_or(MeshError::InvalidFrame)?
                    .iter()
                    .map(|item| {
                        item.as_str()
                            .map(str::to_string)
                            .ok_or(MeshError::InvalidFrame)
                    })
                    .collect()
            })
            .transpose()
            .map(Option::unwrap_or_default)
    };
    let numbers = |name: &str| -> Result<Vec<i32>, MeshError> {
        object
            .get(name)
            .map(|value| {
                value
                    .as_array()
                    .ok_or(MeshError::InvalidFrame)?
                    .iter()
                    .map(|item| {
                        item.as_i64()
                            .and_then(|n| i32::try_from(n).ok())
                            .ok_or(MeshError::InvalidFrame)
                    })
                    .collect()
            })
            .transpose()
            .map(Option::unwrap_or_default)
    };
    Ok(Request {
        ids: strings("ids")?,
        authors: strings("authors")?,
        kinds: numbers("kinds")?,
        since: object
            .get("since")
            .and_then(Value::as_i64)
            .and_then(|n| i32::try_from(n).ok()),
        until: object
            .get("until")
            .and_then(Value::as_i64)
            .and_then(|n| i32::try_from(n).ok()),
        limit: object
            .get("limit")
            .and_then(Value::as_i64)
            .and_then(|n| i32::try_from(n).ok()),
        cache_only: true,
        ..Default::default()
    })
}

fn stable_peer_label(peer_id: &str) -> String {
    hex::encode(&blake3::hash(peer_id.as_bytes()).as_bytes()[..8])
}

fn validate_event(event: &CanonicalEvent) -> Result<(), MeshError> {
    use nipworker_core::crypto::nostr_crypto::{compute_event_id, verify_event_signature};
    use nipworker_core::types::{Event, EventId, PublicKey};

    let pubkey = PublicKey::from_hex(&event.pubkey)
        .map_err(|error| MeshError::InvalidEvent(error.to_string()))?;
    let expected = compute_event_id(
        &pubkey,
        event.created_at,
        event.kind,
        &event.tags,
        &event.content,
    );
    if expected != event.id {
        return Err(MeshError::InvalidEvent("event id mismatch".to_string()));
    }
    let canonical = Event {
        id: EventId::from_hex(&event.id)
            .map_err(|error| MeshError::InvalidEvent(error.to_string()))?,
        pubkey,
        created_at: event.created_at,
        kind: event.kind,
        tags: event.tags.clone(),
        content: event.content.clone(),
        sig: event.sig.clone(),
    };
    verify_event_signature(&canonical).map_err(|error| MeshError::InvalidEvent(error.to_string()))
}
