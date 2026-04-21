use crate::generated::nostr::fb::Request;
use crate::storage::db::index::NostrDB;
use crate::storage::db::sharded_storage::ShardedRingBufferStorage;
use crate::storage::db::types::{QueryFilter, DatabaseError};
use crate::traits::{Storage, StorageError};
use crate::types::nostr::Filter;
use async_trait::async_trait;
use std::sync::Arc;

/// Storage trait implementation that wraps NostrDB for fast in-memory queries.
/// This is the primary cache implementation for the unified engine.
pub struct NostrDbStorage {
    db: Arc<NostrDB<ShardedRingBufferStorage>>,
}

impl NostrDbStorage {
    /// Create a new NostrDbStorage instance
    pub fn new(
        db_name: String,
        max_buffer_size: usize,
        default_relays: Vec<String>,
        indexer_relays: Vec<String>,
    ) -> Self {
        let db = Arc::new(NostrDB::new(
            db_name,
            max_buffer_size,
            default_relays,
            indexer_relays,
        ));
        Self { db }
    }

    /// Get a reference to the underlying NostrDB for advanced operations
    pub fn nostr_db(&self) -> &Arc<NostrDB<ShardedRingBufferStorage>> {
        &self.db
    }

    /// Get a reference to the underlying sharded storage for persistence operations
    pub fn sharded_storage(&self) -> &ShardedRingBufferStorage {
        // NostrDB<ShardedRingBufferStorage> has storage() -> &ShardedRingBufferStorage
        self.db.storage()
    }

    /// Convert nostr Filter to QueryFilter for NostrDB
    fn filter_to_query_filter(filter: &Filter) -> QueryFilter {
        let mut qf = QueryFilter::new();
        
        if let Some(ref ids) = filter.ids {
            qf.ids = Some(ids.iter().map(|id| id.to_string()).collect());
        }
        if let Some(ref authors) = filter.authors {
            qf.authors = Some(authors.iter().map(|pk| pk.to_string()).collect());
        }
        if let Some(ref kinds) = filter.kinds {
            qf.kinds = Some(kinds.clone());
        }
        if let Some(since) = filter.since {
            qf.since = Some(since as u32);
        }
        if let Some(until) = filter.until {
            qf.until = Some(until as u32);
        }
        if let Some(limit) = filter.limit {
            qf.limit = Some(limit as usize);
        }
        if let Some(ref search) = filter.search {
            qf.search = Some(search.clone());
        }
        
        // Handle tags
        if let Some(ref e_tags) = filter.e_tags {
            qf.e_tags = Some(e_tags.clone());
        }
        if let Some(ref p_tags) = filter.p_tags {
            qf.p_tags = Some(p_tags.clone());
        }
        if let Some(ref a_tags) = filter.a_tags {
            qf.a_tags = Some(a_tags.clone());
        }
        if let Some(ref d_tags) = filter.d_tags {
            qf.d_tags = Some(d_tags.clone());
        }
        
        qf
    }

    /// Convert flatbuffers Request to QueryFilter
    fn fb_request_to_query_filter(fb_req: &Request<'_>) -> QueryFilter {
        let mut f = QueryFilter::new();

        // ids
        if let Some(ids) = fb_req.ids() {
            let ids: Vec<String> = ids.iter().map(|s| s.to_string()).collect();
            if !ids.is_empty() {
                f.ids = Some(ids);
            }
        }

        // authors
        if let Some(authors) = fb_req.authors() {
            let authors: Vec<String> = authors.iter().map(|s| s.to_string()).collect();
            if !authors.is_empty() {
                f.authors = Some(authors);
            }
        }

        // kinds
        if let Some(kinds) = fb_req.kinds() {
            let kinds: Vec<u16> = kinds.into_iter().collect();
            if !kinds.is_empty() {
                f.kinds = Some(kinds);
            }
        }

        // since/until/limit
        let since = fb_req.since();
        if since > 0 {
            f.since = Some(since as u32);
        }
        let until = fb_req.until();
        if until > 0 {
            f.until = Some(until as u32);
        }
        let limit = fb_req.limit();
        if limit > 0 {
            f.limit = Some(limit as usize);
        }

        // search
        if let Some(s) = fb_req.search() {
            if !s.is_empty() {
                f.search = Some(s.to_string());
            }
        }

        // tags
        if let Some(tags_vec) = fb_req.tags() {
            for i in 0..tags_vec.len() {
                let sv = tags_vec.get(i);
                if let Some(items) = sv.items() {
                    if items.len() >= 2 {
                        let mut key = items.get(0).to_string();
                        // Strip leading # if present
                        if let Some(stripped) = key.strip_prefix('#') {
                            key = stripped.to_string();
                        }
                        let values: Vec<String> = (1..items.len())
                            .map(|j| items.get(j).to_string())
                            .collect();
                        match key.as_str() {
                            "e" => {
                                if f.e_tags.is_none() {
                                    f.e_tags = Some(Vec::new());
                                }
                                f.e_tags.as_mut().unwrap().extend(values);
                            }
                            "p" => {
                                if f.p_tags.is_none() {
                                    f.p_tags = Some(Vec::new());
                                }
                                f.p_tags.as_mut().unwrap().extend(values);
                            }
                            "a" => {
                                if f.a_tags.is_none() {
                                    f.a_tags = Some(Vec::new());
                                }
                                f.a_tags.as_mut().unwrap().extend(values);
                            }
                            "d" => {
                                if f.d_tags.is_none() {
                                    f.d_tags = Some(Vec::new());
                                }
                                f.d_tags.as_mut().unwrap().extend(values);
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        f
    }
}

#[async_trait(?Send)]
impl Storage for NostrDbStorage {
    async fn query(&self, filters: Vec<Filter>) -> Result<Vec<Vec<u8>>, StorageError> {
        let mut all_events = Vec::new();
        
        for filter in filters {
            let query_filter = Self::filter_to_query_filter(&filter);
            
            match self.db.query_events_with_filter(query_filter) {
                Ok(result) => {
                    all_events.extend(result.events);
                }
                Err(e) => {
                    tracing::warn!("NostrDbStorage query failed: {}", e);
                }
            }
        }
        
        // Sort by created_at (newest first)
        all_events.sort_by(|a, b| {
            let ca = Self::extract_created_at(a).unwrap_or_default();
            let cb = Self::extract_created_at(b).unwrap_or_default();
            cb.cmp(&ca)
        });
        
        Ok(all_events)
    }

    async fn persist(&self, event_bytes: &[u8]) -> Result<(), StorageError> {
        self.db
            .add_worker_message_bytes(event_bytes)
            .await
            .map_err(|e| StorageError::Other(format!("NostrDB persist failed: {}", e)))?;
        Ok(())
    }

    async fn initialize(&self) -> Result<(), StorageError> {
        self.db
            .initialize()
            .await
            .map_err(|e| StorageError::Other(format!("NostrDB initialize failed: {}", e)))?;
        Ok(())
    }
}

impl NostrDbStorage {
    /// Try to extract created_at from event bytes (handles both WorkerMessage and direct events)
    fn extract_created_at(bytes: &[u8]) -> Option<u32> {
        use crate::generated::nostr::fb::{self, WorkerMessage, ParsedEvent, NostrEvent};
        
        // Try WorkerMessage first
        if let Ok(wm) = flatbuffers::root::<WorkerMessage>(bytes) {
            match wm.content_type() {
                fb::Message::ParsedEvent => {
                    if let Some(p) = wm.content_as_parsed_event() {
                        return Some(p.created_at());
                    }
                }
                fb::Message::NostrEvent => {
                    if let Some(n) = wm.content_as_nostr_event() {
                        return Some(n.created_at().max(0) as u32);
                    }
                }
                _ => {}
            }
            return None;
        }
        
        // Legacy format: direct ParsedEvent
        if let Ok(p) = flatbuffers::root::<ParsedEvent>(bytes) {
            return Some(p.created_at());
        }
        
        // Legacy format: direct NostrEvent
        if let Ok(n) = flatbuffers::root::<NostrEvent>(bytes) {
            return Some(n.created_at().max(0) as u32);
        }
        
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_nostr_db_storage_basic() {
        let storage = NostrDbStorage::new(
            "test".to_string(),
            1024 * 1024,
            vec![],
            vec![],
        );
        
        assert!(storage.initialize().await.is_ok());
    }
}
