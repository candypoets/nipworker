use std::collections::BTreeMap;

use crate::db::ring_buffer::RingBufferStorage;
use crate::db::types::{DatabaseConfig, DatabaseError, EventStorage};

/// Upper 8 bits for shard ID, lower 56 bits for inner offset
const SHARD_BITS: u32 = 8;
const INNER_BITS: u32 = 64 - SHARD_BITS;
const INNER_MASK: u64 = (1u64 << INNER_BITS) - 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ShardId {
    Default = 0,
    Kind0 = 1,
    Kind4 = 2,
    Kind7375 = 3,
}

impl ShardId {
    pub fn from_kind(kind: u32) -> Self {
        match kind {
            0 => ShardId::Kind0,
            4 => ShardId::Kind4,
            7375 => ShardId::Kind7375,
            _ => ShardId::Default,
        }
    }
    pub fn as_u8(self) -> u8 {
        self as u8
    }
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(ShardId::Default),
            1 => Some(ShardId::Kind0),
            2 => Some(ShardId::Kind4),
            3 => Some(ShardId::Kind7375),
            _ => None,
        }
    }
}

fn pack_offset(shard: ShardId, inner: u64) -> u64 {
    // Truncate inner to 56 bits to be safe
    ((shard.as_u8() as u64) << INNER_BITS) | (inner & INNER_MASK)
}

fn unpack_offset(composite: u64) -> (ShardId, u64) {
    let shard = ((composite >> INNER_BITS) & 0xff) as u8;
    let inner = composite & INNER_MASK;
    let shard = ShardId::from_u8(shard).unwrap_or(ShardId::Default);
    (shard, inner)
}

/// How to map raw event bytes -> shard. If you already parse events in index.rs,
/// you can avoid re-parsing by using `add_event_for_kind` (see below).
type RouterFn = dyn Fn(&[u8]) -> ShardId;

/// A storage that shards events across multiple ring buffers based on nostr "kind".
pub struct ShardedRingBufferStorage {
    shards: BTreeMap<ShardId, RingBufferStorage>,
    default_shard: ShardId,
    router: Box<RouterFn>,
}

impl ShardedRingBufferStorage {
    /// Construct with explicit shard ring buffers and a router.
    pub fn new(
        shards: BTreeMap<ShardId, RingBufferStorage>,
        default_shard: ShardId,
        router: Box<RouterFn>,
    ) -> Self {
        Self {
            shards,
            default_shard,
            router,
        }
    }

    /// Convenience: build a typical mapping with dedicated rings for kinds 0, 4, 7375 + a default ring.
    pub fn new_default(
        db_name: &str,
        max_buffer_size_default: usize,
        max_buffer_size_kind0: usize,
        max_buffer_size_kind4: usize,
        max_buffer_size_kind7375: usize,
        config: DatabaseConfig,
    ) -> Self {
        let mut shards = BTreeMap::new();

        // Each shard uses a distinct buffer_key, but same IndexedDB db_name.
        shards.insert(
            ShardId::Default,
            RingBufferStorage::new(
                db_name.to_string(),
                "rb:default".to_string(),
                max_buffer_size_default,
                config.clone(),
            ),
        );
        shards.insert(
            ShardId::Kind0,
            RingBufferStorage::new(
                db_name.to_string(),
                "rb:kind:0".to_string(),
                max_buffer_size_kind0,
                config.clone(),
            ),
        );
        shards.insert(
            ShardId::Kind4,
            RingBufferStorage::new(
                db_name.to_string(),
                "rb:kind:4".to_string(),
                max_buffer_size_kind4,
                config.clone(),
            ),
        );
        shards.insert(
            ShardId::Kind7375,
            RingBufferStorage::new(
                db_name.to_string(),
                "rb:kind:7375".to_string(),
                max_buffer_size_kind7375,
                config.clone(),
            ),
        );

        // Default router that tries to sniff "kind" from JSON without pulling in serde.
        // If parsing fails, fall back to default shard.
        let router = Box::new(|bytes: &[u8]| -> ShardId {
            // Extremely lightweight "kind": <number> finder; adjust as needed.
            // This avoids pulling serde_json in wasm builds if you want.
            let s = std::str::from_utf8(bytes).ok();
            if let Some(s) = s {
                if let Some(i) = s.find("\"kind\"") {
                    // naive: find the next digits
                    let tail = &s[i + 6..];
                    if let Some(j) = tail.find(|c: char| c.is_ascii_digit()) {
                        let digits = tail[j..]
                            .chars()
                            .take_while(|c| c.is_ascii_digit())
                            .collect::<String>();
                        if let Ok(k) = digits.parse::<u32>() {
                            return ShardId::from_kind(k);
                        }
                    }
                }
            }
            ShardId::Default
        });

        Self::new(shards, ShardId::Default, router)
    }

    fn shard_for_kind(&self, kind: u32) -> ShardId {
        ShardId::from_kind(kind)
    }

    fn shard_for_bytes(&self, data: &[u8]) -> ShardId {
        (self.router)(data)
    }

    fn get_shard_storage(&self, shard: ShardId) -> Result<&RingBufferStorage, DatabaseError> {
        self.shards
            .get(&shard)
            .ok_or_else(|| DatabaseError::StorageError(format!("Shard {:?} not configured", shard)))
    }

    /// Optional fast-path to avoid re-parsing in the storage layer.
    /// You can downcast to this concrete type in index.rs and call this when you already have the kind.
    pub async fn add_event_for_kind(
        &self,
        kind: u32,
        event_data: &[u8],
    ) -> Result<u64, DatabaseError> {
        let shard = self.shard_for_kind(kind);
        let storage = self.get_shard_storage(shard)?;
        let inner = storage.add_event_data(event_data).await?;
        // Opportunistically persist other shards if they are due.
        self.opportunistic_persist_other_shards(shard).await?;
        Ok(pack_offset(shard, inner))
    }

    async fn opportunistic_persist_other_shards(&self, skip: ShardId) -> Result<(), DatabaseError> {
        // Optionally: do this in parallel with futures::future::join_all
        // Here we do it sequentially to keep it simple/safe for wasm.
        for (id, storage) in &self.shards {
            if *id == skip {
                continue;
            }
            // This only writes when the shard had events and the window elapsed.
            let _ = storage.persist_if_due().await?;
        }
        Ok(())
    }
}

impl EventStorage for ShardedRingBufferStorage {
    async fn initialize_storage(&self) -> Result<(), DatabaseError> {
        for storage in self.shards.values() {
            storage.initialize_storage().await?;
        }
        Ok(())
    }

    async fn add_event_data(&self, event_data: &[u8]) -> Result<u64, DatabaseError> {
        // Route by bytes (Option A). If you downcast and call add_event_for_kind, you'll avoid this parse.
        let shard = self.shard_for_bytes(event_data);
        let storage = self.get_shard_storage(shard)?;
        let inner = storage.add_event_data(event_data).await?;
        // Opportunistically persist other shards if they are due.
        self.opportunistic_persist_other_shards(shard).await?;
        Ok(pack_offset(shard, inner))
    }

    fn get_event(&self, event_offset: u64) -> Result<Option<Vec<u8>>, DatabaseError> {
        let (shard, inner) = unpack_offset(event_offset);
        let storage = self.get_shard_storage(shard)?;
        storage.get_event(inner)
    }

    fn load_events(&self) -> Result<Vec<u64>, DatabaseError> {
        // Aggregate all shards' offsets and tag them with shard ID.
        // NOTE: This returns a flat list without cross-shard ordering guarantees.
        // If you need strict arrival-time ordering across shards, consider maintaining
        // a small additional "commit log" ring that stores only the composite offsets.
        let mut out = Vec::new();
        for (shard, storage) in &self.shards {
            let inner = storage.load_events()?;
            out.extend(inner.into_iter().map(|i| pack_offset(*shard, i)));
        }
        Ok(out)
    }

    async fn clear_storage(&self) -> Result<(), DatabaseError> {
        for storage in self.shards.values() {
            storage.clear_storage().await?;
        }
        Ok(())
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
