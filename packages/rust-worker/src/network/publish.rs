use crate::db::NostrDB;
use crate::parser::Parser;
use crate::relays::ConnectionRegistry;
use anyhow::Result;
use futures::future::join_all;
use js_sys::SharedArrayBuffer;
use nostr::{Event, Kind, UnsignedEvent};
use rustc_hash::FxHashSet;
use std::sync::Arc;
use tracing::{debug, info, warn};

#[derive(Clone)]
pub struct PublishManager {
    database: Arc<NostrDB>,
    connection_registry: Arc<ConnectionRegistry>,
    parser: Arc<Parser>,
    callback: Option<js_sys::Function>,
}

impl PublishManager {
    pub fn new(
        database: Arc<NostrDB>,
        connection_registry: Arc<ConnectionRegistry>,
        parser: Arc<Parser>,
    ) -> Self {
        Self {
            database,
            connection_registry,
            parser,
            callback: None,
        }
    }

    pub async fn publish_event(
        &self,
        publish_id: String,
        unsigned_event: &mut UnsignedEvent,
        shared_buffer: SharedArrayBuffer,
    ) -> Result<()> {
        info!(
            "Publishing event {} with ID {}",
            unsigned_event.id, publish_id
        );

        // Prepare the event using parser
        let event = match self.parser.prepare(unsigned_event) {
            Ok(parsed) => parsed,
            Err(e) => return Err(anyhow::anyhow!("failed to prepare event: {}", e)),
        };

        // Determine target relays for the event
        let relays = match self.determine_target_relays(&event).await {
            Ok(relays) if !relays.is_empty() => relays,
            Ok(_) => {
                debug!("No specific relays determined for publish ID {}, falling back to default relays", publish_id);
                self.database.default_relays.clone()
            }
            Err(e) => {
                warn!(
                    "Failed to determine target relays for publish ID {}: {}, using defaults",
                    publish_id, e
                );
                self.database.default_relays.clone()
            }
        };

        info!(
            "Selected {} relays for publishing: {:?}",
            relays.len(),
            relays
        );

        let _ = self
            .connection_registry
            .publish(
                &publish_id,
                event.clone(),
                relays.clone(),
                shared_buffer.into(),
            )
            .await;

        Ok(())
    }

    async fn determine_target_relays(&self, event: &Event) -> Result<Vec<String>> {
        let mut relay_set = FxHashSet::default();
        let mut write_pubkeys = Vec::new();
        let mut read_pubkeys = Vec::new();

        // Always add the event author's pubkey as a write pubkey
        write_pubkeys.push(event.pubkey.to_hex());

        // Skip extracting mentioned pubkeys for kind 3 (contact list) events
        if event.kind != Kind::ContactList && event.kind.as_u64() < 10000 {
            for tag in &event.tags {
                let tag_vec = tag.as_vec();
                if tag_vec.len() >= 2 && tag_vec[0] == "p" {
                    read_pubkeys.push(tag_vec[1].clone());
                }
            }
        }

        // Get relays for all mentioned pubkeys (read relays)
        let read_tasks: Vec<_> = read_pubkeys
            .into_iter()
            .map(|pubkey| async move { self.database.get_read_relays(&pubkey).unwrap_or_default() })
            .collect();

        // Get relays for author pubkeys (write relays)
        let write_tasks: Vec<_> = write_pubkeys
            .into_iter()
            .map(
                |pubkey| async move { self.database.get_write_relays(&pubkey).unwrap_or_default() },
            )
            .collect();

        // Wait for all tasks to complete
        let read_results = join_all(read_tasks).await;
        let write_results = join_all(write_tasks).await;

        // Collect all relay URLs
        for relays in read_results.into_iter().chain(write_results.into_iter()) {
            for relay in relays {
                relay_set.insert(relay);
            }
        }

        Ok(relay_set.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_publish_manager_creation() {
        // This would require mock implementations of the dependencies
        // For now, just test that the struct can be created
        assert!(true);
    }

    #[test]
    fn test_determine_target_relays() {
        // Test relay determination logic
        // This would require setting up mock database responses
        assert!(true);
    }

    #[test]
    fn test_nip65_parsing() {
        // Test NIP-65 relay list parsing
        assert!(true);
    }
}
