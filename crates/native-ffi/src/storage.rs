use nipworker_core::traits::{Storage, StorageError};
use nipworker_core::types::nostr::Filter;
use std::collections::HashMap;
use tokio::sync::RwLock;

pub struct InMemoryStorage {
    events: RwLock<HashMap<String, Vec<u8>>>,
}

impl InMemoryStorage {
    pub fn new() -> Self {
        Self {
            events: RwLock::new(HashMap::new()),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl Storage for InMemoryStorage {
    async fn query(&self, filters: Vec<Filter>) -> Result<Vec<Vec<u8>>, StorageError> {
        let guard = self.events.read().await;
        let mut results: Vec<(Vec<u8>, u32)> = Vec::new();

        for (_, bytes) in guard.iter() {
            if let Some((kind, pubkey, id, created_at, tags)) = Self::extract_event_fields(bytes) {
                let mut matched = false;
                for filter in &filters {
                    if Self::event_matches_filter(kind, &pubkey, &id, created_at, &tags, filter) {
                        matched = true;
                        break;
                    }
                }
                if matched {
                    results.push((bytes.clone(), created_at));
                }
            }
        }

        // Sort by created_at descending (newest first) for consistency with NostrDbStorage
        results.sort_by(|a, b| b.1.cmp(&a.1));

        Ok(results.into_iter().map(|(bytes, _)| bytes).collect())
    }

    async fn persist(&self, event_bytes: &[u8]) -> Result<(), StorageError> {
        let key = hex::encode(event_bytes);
        let mut guard = self.events.write().await;
        guard.insert(key, event_bytes.to_vec());
        Ok(())
    }

    async fn initialize(&self) -> Result<(), StorageError> {
        Ok(())
    }
}

impl InMemoryStorage {
    /// Extract (kind, pubkey, id, created_at, tags) from stored event bytes.
    /// Handles WorkerMessage wrapper (save_to_db) and raw events.
    fn extract_event_fields(bytes: &[u8]) -> Option<(u16, String, String, u32, Vec<Vec<String>>)> {
        use nipworker_core::generated::nostr::fb;

        // Try WorkerMessage first (save_to_db format)
        if let Ok(wm) = flatbuffers::root::<fb::WorkerMessage>(bytes) {
            match wm.content_type() {
                fb::Message::ParsedEvent => {
                    if let Some(p) = wm.content_as_parsed_event() {
                        return Some((
                            p.kind(),
                            p.pubkey().to_string(),
                            p.id().to_string(),
                            p.created_at(),
                            Self::extract_tags_from_fb(p.tags()),
                        ));
                    }
                }
                fb::Message::NostrEvent => {
                    if let Some(n) = wm.content_as_nostr_event() {
                        return Some((
                            n.kind(),
                            n.pubkey().to_string(),
                            n.id().to_string(),
                            n.created_at().max(0) as u32,
                            Self::extract_tags_from_fb(n.tags()),
                        ));
                    }
                }
                _ => {}
            }
            return None;
        }

        // Legacy: raw ParsedEvent
        if let Ok(p) = flatbuffers::root::<fb::ParsedEvent>(bytes) {
            return Some((
                p.kind(),
                p.pubkey().to_string(),
                p.id().to_string(),
                p.created_at(),
                Self::extract_tags_from_fb(p.tags()),
            ));
        }

        // Legacy: raw NostrEvent
        if let Ok(n) = flatbuffers::root::<fb::NostrEvent>(bytes) {
            return Some((
                n.kind(),
                n.pubkey().to_string(),
                n.id().to_string(),
                n.created_at().max(0) as u32,
                Self::extract_tags_from_fb(n.tags()),
            ));
        }

        None
    }

    fn extract_tags_from_fb(
        tags_fb: flatbuffers::Vector<
            flatbuffers::ForwardsUOffset<nipworker_core::generated::nostr::fb::StringVec>,
        >,
    ) -> Vec<Vec<String>> {
        let mut tags = Vec::new();
        for i in 0..tags_fb.len() {
            let sv = tags_fb.get(i);
            if let Some(items) = sv.items() {
                let tag: Vec<String> = items.iter().map(|s| s.to_string()).collect();
                if !tag.is_empty() {
                    tags.push(tag);
                }
            }
        }
        tags
    }

    /// Check if an event matches a single filter.
    fn event_matches_filter(
        kind: u16,
        pubkey: &str,
        id: &str,
        created_at: u32,
        tags: &[Vec<String>],
        filter: &Filter,
    ) -> bool {
        use nipworker_core::types::nostr::PublicKey;

        // ids filter
        if let Some(ref ids) = filter.ids {
            if !ids.iter().any(|event_id| event_id.to_hex() == id) {
                return false;
            }
        }

        // authors filter
        if let Some(ref authors) = filter.authors {
            if !authors.iter().any(|author| author.to_hex() == pubkey) {
                return false;
            }
        }

        // kinds filter
        if let Some(ref kinds) = filter.kinds {
            if !kinds.contains(&kind) {
                return false;
            }
        }

        // since filter
        if let Some(since) = filter.since {
            if (created_at as u64) < since {
                return false;
            }
        }

        // until filter
        if let Some(until) = filter.until {
            if (created_at as u64) > until {
                return false;
            }
        }

        // e_tags filter (#e)
        if let Some(ref e_tags) = filter.e_tags {
            if !tags
                .iter()
                .any(|t| t.len() >= 2 && t[0] == "e" && e_tags.contains(&t[1]))
            {
                return false;
            }
        }

        // p_tags filter (#p)
        if let Some(ref p_tags) = filter.p_tags {
            if !tags
                .iter()
                .any(|t| t.len() >= 2 && t[0] == "p" && p_tags.contains(&t[1]))
            {
                return false;
            }
        }

        // a_tags filter (#a)
        if let Some(ref a_tags) = filter.a_tags {
            if !tags
                .iter()
                .any(|t| t.len() >= 2 && t[0] == "a" && a_tags.contains(&t[1]))
            {
                return false;
            }
        }

        // d_tags filter (#d)
        if let Some(ref d_tags) = filter.d_tags {
            if !tags
                .iter()
                .any(|t| t.len() >= 2 && t[0] == "d" && d_tags.contains(&t[1]))
            {
                return false;
            }
        }

        // Generic tag filters (includes #q, #t, and any other NIP-defined tags)
        for (tag_key, filter_values) in &filter.tags {
            if !tags
                .iter()
                .any(|t| t.len() >= 2 && &t[0] == tag_key && filter_values.contains(&t[1]))
            {
                return false;
            }
        }

        // Note: search is not implemented for InMemoryStorage.

        true
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use flatbuffers::FlatBufferBuilder;
    use nipworker_core::generated::nostr::fb;
    use nipworker_core::types::nostr::{Filter, PublicKey};

    fn build_worker_message_bytes_with_tags(
        sub_id: &str,
        kind: u16,
        pubkey: &str,
        id: &str,
        created_at: u32,
        tags: Vec<Vec<String>>,
    ) -> Vec<u8> {
        let mut builder = FlatBufferBuilder::new();
        let id_off = builder.create_string(id);
        let pubkey_off = builder.create_string(pubkey);

        let mut tag_offsets = Vec::new();
        for tag in &tags {
            let item_offsets: Vec<_> = tag.iter().map(|s| builder.create_string(s)).collect();
            let items_vec = builder.create_vector(&item_offsets);
            let sv = fb::StringVec::create(
                &mut builder,
                &fb::StringVecArgs {
                    items: Some(items_vec),
                },
            );
            tag_offsets.push(sv);
        }
        let tags_vec = builder.create_vector(&tag_offsets);

        let parsed_event = fb::ParsedEvent::create(
            &mut builder,
            &fb::ParsedEventArgs {
                id: Some(id_off),
                pubkey: Some(pubkey_off),
                kind,
                created_at,
                tags: Some(tags_vec),
                ..Default::default()
            },
        );
        let sub_id_off = builder.create_string(sub_id);
        let wm = fb::WorkerMessage::create(
            &mut builder,
            &fb::WorkerMessageArgs {
                sub_id: Some(sub_id_off),
                content_type: fb::Message::ParsedEvent,
                content: Some(parsed_event.as_union_value()),
                ..Default::default()
            },
        );
        builder.finish(wm, None);
        builder.finished_data().to_vec()
    }

    fn build_worker_message_bytes(
        sub_id: &str,
        kind: u16,
        pubkey: &str,
        id: &str,
        created_at: u32,
    ) -> Vec<u8> {
        build_worker_message_bytes_with_tags(sub_id, kind, pubkey, id, created_at, vec![])
    }

    #[tokio::test]
    async fn test_inmemory_storage_kind_filter_excludes_mismatch() {
        let storage = InMemoryStorage::new();
        let pubkey = "0000000000000000000000000000000000000000000000000000000000000001";
        let id1 = "0000000000000000000000000000000000000000000000000000000000000001";
        let id0 = "0000000000000000000000000000000000000000000000000000000000000002";

        // Persist Kind1 save_to_db WorkerMessage for author X
        let kind1_bytes = build_worker_message_bytes("save_to_db", 1, pubkey, id1, 1000);
        storage.persist(&kind1_bytes).await.unwrap();

        // Query for kinds=[0], authors=[X] -> must return zero events
        let mut filter = Filter::new();
        filter.kinds = Some(vec![0]);
        filter.authors = Some(vec![PublicKey::from_hex(pubkey).unwrap()]);
        let results = storage.query(vec![filter]).await.unwrap();
        assert!(
            results.is_empty(),
            "Kind1 event must NOT match a Kind0 filter"
        );

        // Persist Kind0 for same author
        let kind0_bytes = build_worker_message_bytes("save_to_db", 0, pubkey, id0, 2000);
        storage.persist(&kind0_bytes).await.unwrap();

        // Query for kinds=[0], authors=[X] -> must return exactly one Kind0
        let mut filter = Filter::new();
        filter.kinds = Some(vec![0]);
        filter.authors = Some(vec![PublicKey::from_hex(pubkey).unwrap()]);
        let results = storage.query(vec![filter]).await.unwrap();
        assert_eq!(
            results.len(),
            1,
            "Should return exactly one cached Kind0 event"
        );

        let (kind, _, _, _, _) = InMemoryStorage::extract_event_fields(&results[0]).unwrap();
        assert_eq!(kind, 0, "Returned event must be Kind0");
    }

    #[tokio::test]
    async fn test_inmemory_storage_no_filter_returns_all() {
        let storage = InMemoryStorage::new();
        let pubkey = "0000000000000000000000000000000000000000000000000000000000000001";

        let bytes1 = build_worker_message_bytes("save_to_db", 1, pubkey, "id1", 1000);
        let bytes2 = build_worker_message_bytes("save_to_db", 0, pubkey, "id2", 2000);
        storage.persist(&bytes1).await.unwrap();
        storage.persist(&bytes2).await.unwrap();

        let results = storage.query(vec![Filter::new()]).await.unwrap();
        assert_eq!(results.len(), 2, "Empty filter should return all events");
    }

    #[tokio::test]
    async fn test_inmemory_storage_since_until_filter() {
        let storage = InMemoryStorage::new();
        let pubkey = "0000000000000000000000000000000000000000000000000000000000000001";

        let bytes_old = build_worker_message_bytes("save_to_db", 1, pubkey, "id1", 1000);
        let bytes_new = build_worker_message_bytes("save_to_db", 1, pubkey, "id2", 2000);
        storage.persist(&bytes_old).await.unwrap();
        storage.persist(&bytes_new).await.unwrap();

        let mut filter = Filter::new();
        filter.since = Some(1500);
        let results = storage.query(vec![filter]).await.unwrap();
        assert_eq!(results.len(), 1);
        let (_, _, _, created_at, _) = InMemoryStorage::extract_event_fields(&results[0]).unwrap();
        assert_eq!(created_at, 2000);
    }

    #[tokio::test]
    async fn test_inmemory_storage_id_filter() {
        let storage = InMemoryStorage::new();
        let pubkey = "0000000000000000000000000000000000000000000000000000000000000001";
        let id1 = "00000000000000000000000000000000000000000000000000000000000000a1";
        let id2 = "00000000000000000000000000000000000000000000000000000000000000a2";

        let bytes1 = build_worker_message_bytes("save_to_db", 1, pubkey, id1, 1000);
        let bytes2 = build_worker_message_bytes("save_to_db", 1, pubkey, id2, 1000);
        storage.persist(&bytes1).await.unwrap();
        storage.persist(&bytes2).await.unwrap();

        use nipworker_core::types::nostr::EventId;
        let mut filter = Filter::new();
        filter.ids = Some(vec![EventId::from_hex(id2).unwrap()]);
        let results = storage.query(vec![filter]).await.unwrap();
        assert_eq!(results.len(), 1);
        let (_, _, id, _, _) = InMemoryStorage::extract_event_fields(&results[0]).unwrap();
        assert_eq!(id, id2);
    }

    #[tokio::test]
    async fn test_inmemory_storage_q_tag_filter() {
        let storage = InMemoryStorage::new();
        let pubkey = "0000000000000000000000000000000000000000000000000000000000000001";
        let quoted_id = "00000000000000000000000000000000000000000000000000000000000000ab";

        // Event with a q tag quoting quoted_id
        let with_q = build_worker_message_bytes_with_tags(
            "save_to_db",
            1,
            pubkey,
            "event_with_q",
            1000,
            vec![vec!["q".to_string(), quoted_id.to_string()]],
        );
        storage.persist(&with_q).await.unwrap();

        // Event without a q tag
        let without_q = build_worker_message_bytes_with_tags(
            "save_to_db",
            1,
            pubkey,
            "event_without_q",
            1000,
            vec![vec!["e".to_string(), "some_other_id".to_string()]],
        );
        storage.persist(&without_q).await.unwrap();

        // Query for q tag -> should return only the event with the q tag
        let mut filter = Filter::new();
        filter
            .tags
            .insert("q".to_string(), vec![quoted_id.to_string()]);
        let results = storage.query(vec![filter]).await.unwrap();
        assert_eq!(
            results.len(),
            1,
            "Should return exactly one event with matching q tag"
        );

        let (_, _, id, _, _) = InMemoryStorage::extract_event_fields(&results[0]).unwrap();
        assert_eq!(id, "event_with_q");
    }

    #[tokio::test]
    async fn test_inmemory_storage_e_tag_filter() {
        let storage = InMemoryStorage::new();
        let pubkey = "0000000000000000000000000000000000000000000000000000000000000001";
        let e_id = "00000000000000000000000000000000000000000000000000000000000000ef";

        let with_e = build_worker_message_bytes_with_tags(
            "save_to_db",
            1,
            pubkey,
            "event_with_e",
            1000,
            vec![vec![
                "e".to_string(),
                e_id.to_string(),
                "wss://relay.example.com".to_string(),
            ]],
        );
        storage.persist(&with_e).await.unwrap();

        let without_e =
            build_worker_message_bytes("save_to_db", 1, pubkey, "event_without_e", 1000);
        storage.persist(&without_e).await.unwrap();

        let mut filter = Filter::new();
        filter.e_tags = Some(vec![e_id.to_string()]);
        let results = storage.query(vec![filter]).await.unwrap();
        assert_eq!(
            results.len(),
            1,
            "Should return exactly one event with matching e tag"
        );

        let (_, _, id, _, _) = InMemoryStorage::extract_event_fields(&results[0]).unwrap();
        assert_eq!(id, "event_with_e");
    }

    #[tokio::test]
    async fn test_inmemory_storage_uppercase_nip22_tag_filter() {
        let storage = InMemoryStorage::new();
        let pubkey = "0000000000000000000000000000000000000000000000000000000000000001";
        let parent_id = "00000000000000000000000000000000000000000000000000000000000000ab";
        let parent_pubkey = "00000000000000000000000000000000000000000000000000000000000000cd";

        // NIP-22 comment event with uppercase E, P, K tags
        let nip22_event = build_worker_message_bytes_with_tags(
            "save_to_db",
            1111,
            pubkey,
            "nip22_comment",
            1000,
            vec![
                vec!["E".to_string(), parent_id.to_string()],
                vec!["P".to_string(), parent_pubkey.to_string()],
                vec!["K".to_string(), "1".to_string()],
                vec![
                    "e".to_string(),
                    parent_id.to_string(),
                    "wss://relay.example.com".to_string(),
                ],
            ],
        );
        storage.persist(&nip22_event).await.unwrap();

        // Regular kind-1 note without uppercase tags
        let regular_note = build_worker_message_bytes_with_tags(
            "save_to_db",
            1,
            pubkey,
            "regular_note",
            1000,
            vec![vec!["e".to_string(), "some_other_id".to_string()]],
        );
        storage.persist(&regular_note).await.unwrap();

        // Query for uppercase #E -> should match only the NIP-22 event
        let mut filter = Filter::new();
        filter
            .tags
            .insert("E".to_string(), vec![parent_id.to_string()]);
        let results = storage.query(vec![filter]).await.unwrap();
        assert_eq!(
            results.len(),
            1,
            "Uppercase #E filter should match only NIP-22 event"
        );
        let (_, _, id, _, _) = InMemoryStorage::extract_event_fields(&results[0]).unwrap();
        assert_eq!(id, "nip22_comment");

        // Query for uppercase #P -> same
        let mut filter = Filter::new();
        filter
            .tags
            .insert("P".to_string(), vec![parent_pubkey.to_string()]);
        let results = storage.query(vec![filter]).await.unwrap();
        assert_eq!(results.len(), 1);

        // Query for lowercase #e -> both events match (NIP-22 has lowercase e too)
        let mut filter = Filter::new();
        filter.e_tags = Some(vec![parent_id.to_string()]);
        let results = storage.query(vec![filter]).await.unwrap();
        assert_eq!(
            results.len(),
            1,
            "Lowercase #e should match only the event with lowercase e tag"
        );
        let (_, _, id, _, _) = InMemoryStorage::extract_event_fields(&results[0]).unwrap();
        assert_eq!(id, "nip22_comment");
    }
}
