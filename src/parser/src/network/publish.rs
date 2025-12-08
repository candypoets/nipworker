use crate::nostr::Template;
use crate::parser::Parser;
use crate::types::nostr::Event;
use crate::NostrError;
use std::sync::Arc;

type Result<T> = std::result::Result<T, NostrError>;
use tracing::info;

#[derive(Clone)]
pub struct PublishManager {
    parser: Arc<Parser>,
}

impl PublishManager {
    pub fn new(parser: Arc<Parser>) -> Self {
        Self { parser }
    }

    pub async fn publish_event(
        &self,
        publish_id: String,
        template: &Template,
    ) -> Result<(Event, Vec<String>)> {
        info!("Publishing event with ID {}", publish_id);

        // Prepare the event using parser
        let event = match self.parser.prepare(template) {
            Ok(parsed) => parsed,
            Err(e) => return Err(NostrError::Other(format!("failed to prepare event: {}", e))),
        };

        // Determine target relays for the event
        // let relays = match self.determine_target_relays(&event).await {
        //     Ok(relays) if !relays.is_empty() => relays,
        //     Ok(_) => self.database.default_relays.clone(),
        //     Err(e) => {
        //         warn!(
        //             "Failed to determine target relays for publish ID {}: {}, using defaults",
        //             publish_id, e
        //         );
        //         self.database.default_relays.clone()
        //     }
        // };
        //
        let relays: Vec<String> = Vec::new();

        info!(
            "Selected {} relays for publishing: {:?}",
            relays.len(),
            relays
        );

        Ok((event, relays))
    }

    // async fn determine_target_relays(&self, event: &Event) -> Result<Vec<String>> {
    //     let mut relay_set = FxHashSet::default();
    //     let mut write_pubkeys = Vec::new();
    //     let mut read_pubkeys = Vec::new();

    //     // Always add the event author's pubkey as a write pubkey
    //     write_pubkeys.push(event.pubkey.to_hex());

    //     // Skip extracting mentioned pubkeys for kind 3 (contact list) events
    //     if event.kind != CONTACT_LIST && event.kind < 10000 {
    //         for tag in &event.tags {
    //             if tag.len() >= 2 && tag[0] == "p" {
    //                 read_pubkeys.push(tag[1].clone());
    //             }
    //         }
    //     }

    //     // Get relays for all mentioned pubkeys (read relays)
    //     let read_tasks: Vec<_> = read_pubkeys
    //         .into_iter()
    //         .map(|pubkey| async move { self.database.get_read_relays(&pubkey).unwrap_or_default() })
    //         .collect();

    //     // Get relays for author pubkeys (write relays)
    //     let write_tasks: Vec<_> = write_pubkeys
    //         .into_iter()
    //         .map(
    //             |pubkey| async move { self.database.get_write_relays(&pubkey).unwrap_or_default() },
    //         )
    //         .collect();

    //     // Wait for all tasks to complete
    //     let read_results = join_all(read_tasks).await;
    //     let write_results = join_all(write_tasks).await;

    //     // Collect all relay URLs
    //     for relays in read_results.into_iter().chain(write_results.into_iter()) {
    //         for relay in relays {
    //             relay_set.insert(relay);
    //         }
    //     }

    //     Ok(relay_set.into_iter().collect())
    // }
}
