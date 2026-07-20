//! Persistent per-peer NIP-77 session state.

use std::collections::HashMap;

use negentropy::{Id, Negentropy, NegentropyStorageVector};
use serde_json::Value;

use crate::{
    negentropy_storage, MeshError, MeshEventStore, NostrFrame, NEGENTROPY_FRAME_SIZE_LIMIT,
};

type Reconciler = Negentropy<'static, NegentropyStorageVector>;

enum SyncState {
    Initiator { reconciler: Reconciler },
    Responder { reconciler: Reconciler },
}

#[derive(Default)]
pub struct PeerSession {
    syncs: HashMap<String, SyncState>,
}

#[derive(Default)]
pub struct SessionOutput {
    pub frames: Vec<NostrFrame>,
    pub completed: Option<SyncDifference>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncDifference {
    pub have: Vec<String>,
    pub need: Vec<String>,
}

impl PeerSession {
    pub fn active_sync_count(&self) -> usize {
        self.syncs.len()
    }

    pub fn begin_sync(
        &mut self,
        subscription_id: &str,
        filter: Value,
        store: &impl MeshEventStore,
    ) -> Result<NostrFrame, MeshError> {
        let storage = negentropy_storage(store)?;
        let mut reconciler = Negentropy::owned(storage, NEGENTROPY_FRAME_SIZE_LIMIT)?;
        let initial = reconciler.initiate()?;
        self.syncs.insert(
            subscription_id.to_string(),
            SyncState::Initiator { reconciler },
        );
        Ok(NostrFrame::neg_open(subscription_id, &filter, &initial))
    }

    pub fn handle(
        &mut self,
        frame: &NostrFrame,
        store: &impl MeshEventStore,
    ) -> Result<SessionOutput, MeshError> {
        let value: Value = serde_json::from_str(frame.as_str())?;
        let array = value.as_array().ok_or(MeshError::InvalidFrame)?;
        let kind = array
            .first()
            .and_then(Value::as_str)
            .ok_or(MeshError::InvalidFrame)?;
        let subscription_id = array
            .get(1)
            .and_then(Value::as_str)
            .ok_or(MeshError::InvalidFrame)?;

        match kind {
            "NEG-OPEN" => {
                let payload = decode_payload(array.get(3))?;
                let storage = negentropy_storage(store)?;
                let mut reconciler = Negentropy::owned(storage, NEGENTROPY_FRAME_SIZE_LIMIT)?;
                let response = reconciler.reconcile(&payload)?;
                self.syncs.insert(
                    subscription_id.to_string(),
                    SyncState::Responder { reconciler },
                );
                Ok(SessionOutput {
                    frames: vec![NostrFrame::neg_msg(subscription_id, &response)],
                    completed: None,
                })
            }
            "NEG-MSG" => {
                let payload = decode_payload(array.get(2))?;
                let state = self
                    .syncs
                    .get_mut(subscription_id)
                    .ok_or_else(|| MeshError::UnknownSession(subscription_id.to_string()))?;
                match state {
                    SyncState::Initiator { reconciler } => {
                        let mut have = Vec::new();
                        let mut need = Vec::new();
                        match reconciler.reconcile_with_ids(&payload, &mut have, &mut need)? {
                            Some(next) => Ok(SessionOutput {
                                frames: vec![NostrFrame::neg_msg(subscription_id, &next)],
                                completed: None,
                            }),
                            None => {
                                self.syncs.remove(subscription_id);
                                Ok(SessionOutput {
                                    frames: vec![NostrFrame::neg_close(subscription_id)],
                                    completed: Some(SyncDifference {
                                        have: ids_to_hex(have),
                                        need: ids_to_hex(need),
                                    }),
                                })
                            }
                        }
                    }
                    SyncState::Responder { reconciler } => {
                        let response = reconciler.reconcile(&payload)?;
                        Ok(SessionOutput {
                            frames: vec![NostrFrame::neg_msg(subscription_id, &response)],
                            completed: None,
                        })
                    }
                }
            }
            "NEG-CLOSE" => {
                self.syncs.remove(subscription_id);
                Ok(SessionOutput::default())
            }
            _ => Err(MeshError::UnexpectedSessionMessage(kind.to_string())),
        }
    }
}

fn decode_payload(value: Option<&Value>) -> Result<Vec<u8>, MeshError> {
    let encoded = value
        .and_then(Value::as_str)
        .ok_or(MeshError::InvalidFrame)?;
    hex::decode(encoded).map_err(|_| MeshError::InvalidFrame)
}

fn ids_to_hex(ids: Vec<Id>) -> Vec<String> {
    ids.into_iter()
        .map(|id| hex::encode(id.to_bytes()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CanonicalEvent, MemoryEventStore};

    fn event(byte: u8, timestamp: u64) -> CanonicalEvent {
        CanonicalEvent {
            id: hex::encode([byte; 32]),
            pubkey: hex::encode([byte.wrapping_add(1); 32]),
            created_at: timestamp,
            kind: 0,
            tags: vec![],
            content: "{}".to_string(),
            sig: hex::encode([0x55; 64]),
        }
    }

    #[test]
    fn session_state_survives_each_neg_msg_until_completion() {
        let mut a_store = MemoryEventStore::default();
        let mut b_store = MemoryEventStore::default();
        assert!(a_store.persist(event(0x11, 100)));
        assert!(b_store.persist(event(0x22, 200)));
        let mut a = PeerSession::default();
        let mut b = PeerSession::default();

        let mut frame = a
            .begin_sync("sync", serde_json::json!({ "kinds": [0] }), &a_store)
            .unwrap();
        assert_eq!(a.active_sync_count(), 1);
        let difference = loop {
            let b_output = b.handle(&frame, &b_store).unwrap();
            assert_eq!(b.active_sync_count(), 1);
            frame = b_output.frames.into_iter().next().unwrap();
            let a_output = a.handle(&frame, &a_store).unwrap();
            if let Some(difference) = a_output.completed {
                let close = a_output.frames.into_iter().next().unwrap();
                b.handle(&close, &b_store).unwrap();
                break difference;
            }
            assert_eq!(a.active_sync_count(), 1);
            frame = a_output.frames.into_iter().next().unwrap();
        };

        assert_eq!(difference.have, vec![hex::encode([0x11; 32])]);
        assert_eq!(difference.need, vec![hex::encode([0x22; 32])]);
        assert_eq!(a.active_sync_count(), 0);
        assert_eq!(b.active_sync_count(), 0);
    }

    #[test]
    fn disconnect_can_drop_all_inflight_state() {
        let store = MemoryEventStore::default();
        let mut session = PeerSession::default();
        session
            .begin_sync("one", serde_json::json!({}), &store)
            .unwrap();
        session
            .begin_sync("two", serde_json::json!({}), &store)
            .unwrap();
        assert_eq!(session.active_sync_count(), 2);

        session = PeerSession::default();
        assert_eq!(session.active_sync_count(), 0);
    }
}
