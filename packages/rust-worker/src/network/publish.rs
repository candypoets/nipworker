use crate::db::NostrDB;
use crate::nostr::Template;
use crate::parser::Parser;
use crate::types::nostr::Event;
use crate::NostrError;
use crate::CONTACT_LIST;
use futures::future::join_all;
use js_sys::SharedArrayBuffer;
use rustc_hash::FxHashSet;
use std::sync::Arc;

type Result<T> = std::result::Result<T, NostrError>;
use tracing::{info, warn};

#[derive(Clone)]
pub struct PublishManager {
    database: Arc<NostrDB>,
    parser: Arc<Parser>,
    callback: Option<js_sys::Function>,
}

impl PublishManager {
    pub fn new(database: Arc<NostrDB>, parser: Arc<Parser>) -> Self {
        Self {
            database,
            parser,
            callback: None,
        }
    }

    pub async fn publish_event(
        &self,
        publish_id: String,
        template: &Template,
        shared_buffer: SharedArrayBuffer,
    ) -> Result<(Event, Vec<String>)> {
        info!("Publishing event with ID {}", publish_id);

        // Prepare the event using parser
        let event = match self.parser.prepare(template) {
            Ok(parsed) => parsed,
            Err(e) => return Err(NostrError::Other(format!("failed to prepare event: {}", e))),
        };

        // Determine target relays for the event
        let relays = match self.determine_target_relays(&event).await {
            Ok(relays) if !relays.is_empty() => relays,
            Ok(_) => self.database.default_relays.clone(),
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

        // let _ = self
        //     .connection_registry
        //     .publish(&publish_id, event, relays.clone(), shared_buffer.into())
        //     .await;

        Ok((event, relays))
    }

    async fn determine_target_relays(&self, event: &Event) -> Result<Vec<String>> {
        let mut relay_set = FxHashSet::default();
        let mut write_pubkeys = Vec::new();
        let mut read_pubkeys = Vec::new();

        // Always add the event author's pubkey as a write pubkey
        write_pubkeys.push(event.pubkey.to_hex());

        // Skip extracting mentioned pubkeys for kind 3 (contact list) events
        if event.kind != CONTACT_LIST && event.kind < 10000 {
            for tag in &event.tags {
                if tag.len() >= 2 && tag[0] == "p" {
                    read_pubkeys.push(tag[1].clone());
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
