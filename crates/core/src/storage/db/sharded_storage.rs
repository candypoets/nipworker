use std::collections::BTreeMap;

use crate::storage::db::ring_buffer::RingBufferStorage;
use crate::storage::db::types::{DatabaseConfig, DatabaseError, EventStorage};

/// Upper 8 bits for shard ID, lower 56 bits for inner offset
const SHARD_BITS: u32 = 8;
const INNER_BITS: u32 = 64 - SHARD_BITS;
const INNER_MASK: u64 = (1u64 << INNER_BITS) - 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ShardId {
    Default = 0,
    Profile = 1,
    Kind4 = 2,
    Kind7375 = 3,
    Replaceable = 4,
    Regular = 5,
    Reaction = 6,
    Ephemeral = 7,
    Addressable = 8,
}

impl ShardId {
    pub fn from_kind(kind: u32) -> Self {
        match kind {
            0 => ShardId::Profile,
            3 => ShardId::Replaceable,
            4 => ShardId::Kind4,
            7 => ShardId::Reaction,
            7375 => ShardId::Kind7375,
            1..=9999 => ShardId::Regular,
            10000..=19999 => ShardId::Replaceable,
            20000..=29999 => ShardId::Ephemeral,
            30000..=39999 => ShardId::Addressable,
            _ => ShardId::Default,
        }
    }

    pub fn as_u8(self) -> u8 {
        self as u8
    }

    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(ShardId::Default),
            1 => Some(ShardId::Profile),
            2 => Some(ShardId::Kind4),
            3 => Some(ShardId::Kind7375),
            4 => Some(ShardId::Replaceable),
            5 => Some(ShardId::Regular),
            6 => Some(ShardId::Reaction),
            7 => Some(ShardId::Ephemeral),
            8 => Some(ShardId::Addressable),
            _ => None,
        }
    }

    /// Ephemeral events remain queryable for the current session but must not
    /// survive a restart. All other shards are backed by the configured blob store.
    pub fn is_persistent(self) -> bool {
        self != ShardId::Ephemeral
    }

    pub const fn persistent_ids() -> &'static [ShardId] {
        &[
            ShardId::Default,
            ShardId::Profile,
            ShardId::Kind4,
            ShardId::Kind7375,
            ShardId::Replaceable,
            ShardId::Regular,
            ShardId::Reaction,
            ShardId::Addressable,
        ]
    }

    pub fn persistence_key(self) -> Option<&'static str> {
        match self {
            ShardId::Default => Some("shard:default"),
            ShardId::Profile => Some("shard:kind0"),
            ShardId::Kind4 => Some("shard:kind4"),
            ShardId::Kind7375 => Some("shard:kind7375"),
            ShardId::Replaceable => Some("shard:replaceable"),
            ShardId::Regular => Some("shard:regular"),
            ShardId::Reaction => Some("shard:reaction"),
            ShardId::Addressable => Some("shard:addressable"),
            ShardId::Ephemeral => None,
        }
    }
}

/// Capacity split for the nine runtime shards. The weights sum to 28 and are
/// deliberately applied to the old total budget (the caller-provided default
/// capacity plus the former 6 MiB of dedicated rings), so adding shards does
/// not increase total reserved memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ShardCapacities {
    default: usize,
    profile: usize,
    kind4: usize,
    kind7375: usize,
    replaceable: usize,
    regular: usize,
    reaction: usize,
    ephemeral: usize,
    addressable: usize,
}

impl ShardCapacities {
    const LEGACY_DEDICATED_BYTES: usize = 6 * 1024 * 1024;
    const TOTAL_WEIGHT: usize = 28;

    fn from_legacy_default(default_capacity: usize) -> Self {
        let total = default_capacity.saturating_add(Self::LEGACY_DEDICATED_BYTES);
        let weighted = |weight: usize| {
            (total / Self::TOTAL_WEIGHT) * weight
                + ((total % Self::TOTAL_WEIGHT) * weight) / Self::TOTAL_WEIGHT
        };

        let default = weighted(1);
        let profile = weighted(2);
        let kind4 = weighted(2);
        let kind7375 = weighted(1);
        let replaceable = weighted(4);
        let regular = weighted(8);
        let reaction = weighted(3);
        let ephemeral = weighted(2);
        let allocated =
            default + profile + kind4 + kind7375 + replaceable + regular + reaction + ephemeral;

        Self {
            default,
            profile,
            kind4,
            kind7375,
            replaceable,
            regular,
            reaction,
            ephemeral,
            // Give division remainder to the large addressable-event shard.
            addressable: total - allocated,
        }
    }

    #[cfg(test)]
    fn total(self) -> usize {
        self.default
            + self.profile
            + self.kind4
            + self.kind7375
            + self.replaceable
            + self.regular
            + self.reaction
            + self.ephemeral
            + self.addressable
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

    /// Build the standard semantic shard layout without increasing the legacy
    /// aggregate cache budget.
    pub fn new_default(
        db_name: &str,
        legacy_default_capacity: usize,
        config: DatabaseConfig,
    ) -> Self {
        let mut shards = BTreeMap::new();
        let capacities = ShardCapacities::from_legacy_default(legacy_default_capacity);

        // Each shard uses a distinct buffer_key, but same IndexedDB db_name.
        shards.insert(
            ShardId::Default,
            RingBufferStorage::new(
                db_name.to_string(),
                "rb:default".to_string(),
                capacities.default,
                config.clone(),
            ),
        );
        shards.insert(
            ShardId::Profile,
            RingBufferStorage::new(
                db_name.to_string(),
                "rb:kind:0".to_string(),
                capacities.profile,
                config.clone(),
            ),
        );
        shards.insert(
            ShardId::Kind4,
            RingBufferStorage::new(
                db_name.to_string(),
                "rb:kind:4".to_string(),
                capacities.kind4,
                config.clone(),
            ),
        );
        shards.insert(
            ShardId::Kind7375,
            RingBufferStorage::new(
                db_name.to_string(),
                "rb:kind:7375".to_string(),
                capacities.kind7375,
                config.clone(),
            ),
        );
        shards.insert(
            ShardId::Replaceable,
            RingBufferStorage::new(
                db_name.to_string(),
                "rb:replaceable".to_string(),
                capacities.replaceable,
                config.clone(),
            ),
        );
        shards.insert(
            ShardId::Regular,
            RingBufferStorage::new(
                db_name.to_string(),
                "rb:regular".to_string(),
                capacities.regular,
                config.clone(),
            ),
        );
        shards.insert(
            ShardId::Reaction,
            RingBufferStorage::new(
                db_name.to_string(),
                "rb:reaction".to_string(),
                capacities.reaction,
                config.clone(),
            ),
        );
        shards.insert(
            ShardId::Ephemeral,
            RingBufferStorage::new(
                db_name.to_string(),
                "rb:ephemeral".to_string(),
                capacities.ephemeral,
                config.clone(),
            ),
        );
        shards.insert(
            ShardId::Addressable,
            RingBufferStorage::new(
                db_name.to_string(),
                "rb:addressable".to_string(),
                capacities.addressable,
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

    pub fn shard_for_kind(&self, kind: u32) -> ShardId {
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
        Ok(pack_offset(shard, inner))
    }
}

impl ShardedRingBufferStorage {
    /// Save all shards' buffer contents to a HashMap.
    /// Returns a map from ShardId to raw bytes for each shard.
    pub fn save_all_shards(&self) -> std::collections::HashMap<ShardId, Vec<u8>> {
        let mut result = std::collections::HashMap::new();
        for (shard_id, storage) in &self.shards {
            if !shard_id.is_persistent() {
                continue;
            }
            let bytes = storage.save_to_bytes();
            if !bytes.is_empty() {
                result.insert(*shard_id, bytes);
            }
        }
        result
    }

    /// Load all shards from a HashMap of bytes.
    /// The input should come from a previous `save_all_shards` call.
    pub fn load_all_shards(
        &self,
        shard_bytes: &std::collections::HashMap<ShardId, Vec<u8>>,
    ) -> Result<(), DatabaseError> {
        for (shard_id, bytes) in shard_bytes {
            if !shard_id.is_persistent() {
                continue;
            }
            let storage = self.get_shard_storage(*shard_id)?;
            storage
                .load_from_bytes(bytes)
                .map_err(|e| DatabaseError::StorageError(format!("Shard {:?}: {}", shard_id, e)))?;
        }
        Ok(())
    }

    /// Get the db_name used by all shards (for IndexedDB persistence).
    pub fn db_name(&self) -> &str {
        // All shards share the same db_name, get from the first one
        self.shards
            .values()
            .next()
            .map(|s| s.db_name())
            .unwrap_or("")
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
        Ok(pack_offset(shard, inner))
    }

    fn get_event(&self, event_offset: u64) -> Result<Option<Vec<u8>>, DatabaseError> {
        let (shard, inner) = unpack_offset(event_offset);
        let storage = self.get_shard_storage(shard)?;
        storage.get_event(inner)
    }

    fn contains_offset(&self, event_offset: u64) -> bool {
        let (shard, inner) = unpack_offset(event_offset);
        match self.shards.get(&shard) {
            Some(storage) => storage.contains_offset(inner),
            None => false,
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn storage(default_capacity: usize) -> ShardedRingBufferStorage {
        ShardedRingBufferStorage::new_default(
            "test-db",
            default_capacity,
            DatabaseConfig::default(),
        )
    }

    #[test]
    fn routes_kinds_to_semantic_shards() {
        let storage = storage(8 * 1024 * 1024);

        assert_eq!(storage.shard_for_kind(0), ShardId::Profile);
        assert_eq!(storage.shard_for_kind(1), ShardId::Regular);
        assert_eq!(storage.shard_for_kind(3), ShardId::Replaceable);
        assert_eq!(storage.shard_for_kind(4), ShardId::Kind4);
        assert_eq!(storage.shard_for_kind(6), ShardId::Regular);
        assert_eq!(storage.shard_for_kind(7), ShardId::Reaction);
        assert_eq!(storage.shard_for_kind(7375), ShardId::Kind7375);
        assert_eq!(storage.shard_for_kind(10000), ShardId::Replaceable);
        assert_eq!(storage.shard_for_kind(10002), ShardId::Replaceable);
        assert_eq!(storage.shard_for_kind(19999), ShardId::Replaceable);
        assert_eq!(storage.shard_for_kind(20000), ShardId::Ephemeral);
        assert_eq!(storage.shard_for_kind(29999), ShardId::Ephemeral);
        assert_eq!(storage.shard_for_kind(30000), ShardId::Addressable);
        assert_eq!(storage.shard_for_kind(39999), ShardId::Addressable);
        assert_eq!(storage.shard_for_kind(40000), ShardId::Default);
    }

    #[test]
    fn repartitions_without_growing_the_legacy_budget() {
        let default_capacity = 8 * 1024 * 1024;
        let capacities = ShardCapacities::from_legacy_default(default_capacity);

        assert_eq!(
            capacities.total(),
            default_capacity + ShardCapacities::LEGACY_DEDICATED_BYTES
        );
    }

    #[tokio::test]
    async fn ephemeral_events_are_not_in_persistence_snapshot() {
        let storage = storage(8 * 1024 * 1024);
        storage.initialize_storage().await.unwrap();
        storage
            .add_event_for_kind(20001, b"ephemeral")
            .await
            .unwrap();
        storage
            .add_event_for_kind(30001, b"addressable")
            .await
            .unwrap();

        let snapshot = storage.save_all_shards();
        assert!(!snapshot.contains_key(&ShardId::Ephemeral));
        assert!(snapshot.contains_key(&ShardId::Addressable));
    }
}
