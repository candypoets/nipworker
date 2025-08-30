use crate::db::NostrDB;
use crate::network::interfaces::{CacheProcessor as CacheProcessorTrait, EventDatabase};
use crate::parser::Parser;
use crate::types::network::Request;
use crate::types::*;
use anyhow::Result;
use std::collections::HashSet;
use std::sync::Arc;
use tracing::{debug, warn};

pub struct CacheProcessor {
    database: Arc<NostrDB>,
    parser: Arc<Parser>,
}

impl CacheProcessor {
    pub fn new(database: Arc<NostrDB>, parser: Arc<Parser>) -> Self {
        Self { database, parser }
    }

    pub async fn find_context_events_simple(
        &self,
        event: &ParsedEvent,
        max_depth: usize,
    ) -> Vec<ParsedEvent> {
        let mut context_events = Vec::new();
        let mut visited = HashSet::<String>::new();

        self.find_event_context_recursive(event, &mut context_events, 0, max_depth, &mut visited)
            .await;

        context_events
    }

    async fn find_event_context_recursive(
        &self,
        event: &ParsedEvent,
        context: &mut Vec<ParsedEvent>,
        depth: usize,
        max_depth: usize,
        visited: &mut HashSet<String>,
    ) {
        if depth > max_depth {
            return;
        }

        // Avoid infinite loops by tracking visited events
        if visited.contains(&event.event.id.to_hex()) {
            return;
        }
        visited.insert(event.event.id.to_hex());

        // Process requests from this event (matching Go implementation)
        if let Some(requests) = &event.requests {
            for request in requests {
                if let Ok(filter) = request.to_filter() {
                    if let Ok(related_events) =
                        EventDatabase::query_events(&*self.database, filter).await
                    {
                        for related_event in related_events {
                            context.push(related_event.clone());
                            Box::pin(self.find_event_context_recursive(
                                &related_event,
                                context,
                                depth + 1,
                                max_depth,
                                visited,
                            ))
                            .await;
                        }
                    }
                }
            }
        }
    }

    fn create_event_filter(&self, event_id: &str) -> Result<nostr::Filter> {
        let event_id = nostr::EventId::from_hex(event_id)?;
        Ok(nostr::Filter::new().id(event_id))
    }

    fn create_profile_filter(&self, pubkey: &str) -> Result<nostr::Filter> {
        let pubkey = nostr::PublicKey::from_hex(pubkey)?;
        Ok(nostr::Filter::new()
            .author(pubkey)
            .kind(nostr::Kind::Metadata))
    }

    fn create_address_filter(&self, address: &str) -> Result<nostr::Filter> {
        // Parse address format: kind:pubkey:d_tag
        let parts: Vec<&str> = address.split(':').collect();
        if parts.len() != 3 {
            return Err(anyhow::anyhow!("Invalid address format"));
        }

        let kind = parts[0].parse::<u64>()?;
        let pubkey = nostr::PublicKey::from_hex(parts[1])?;
        let d_tag = parts[2];

        Ok(nostr::Filter::new()
            .kind(nostr::Kind::from(kind))
            .author(pubkey)
            .custom_tag(
                nostr::SingleLetterTag::lowercase(nostr::Alphabet::D),
                vec![d_tag],
            ))
    }
}

impl CacheProcessorTrait for CacheProcessor {
    async fn process_local_requests(
        &self,
        requests: Vec<Request>,
        max_depth: usize,
    ) -> Result<(Vec<Request>, Vec<Vec<ParsedEvent>>)> {
        debug!(
            "Processing {} local requests with max depth {}",
            requests.len(),
            max_depth
        );

        let mut remaining_requests = Vec::new();
        let mut cached_events_batches = Vec::new();

        for request in requests {
            // Convert request to filter and query local database
            match request.to_filter() {
                Ok(filter) => {
                    match EventDatabase::query_events(&*self.database, filter).await {
                        Ok(events) => {
                            if events.is_empty() {
                                debug!("No cached events found for request");
                                // No cached events, add to remaining requests
                                remaining_requests.push(request);
                                // cached_events_batches.push(Vec::new());
                            } else {
                                debug!("Found {} cached events for request", events.len());
                                // Found cached events
                                // Process each event and build context like Go implementation
                                let mut processed_events: Vec<Vec<ParsedEvent>> = Vec::new();

                                for event in &events {
                                    // Store the main event
                                    let events_with_context = vec![event.clone()];

                                    // Handle recursive requests from parsed event
                                    // if !request.no_context
                                    //     && event.requests.is_some()
                                    //     && !event.requests.as_ref().unwrap().is_empty()
                                    // {
                                    //     let context_events =
                                    //         self.find_context_events_simple(event, 3).await;
                                    //     events_with_context.extend(context_events);
                                    // }

                                    processed_events.push(events_with_context);
                                }

                                cached_events_batches.extend(processed_events);

                                // Check if we need to fetch more from network
                                if !request.cache_first {
                                    remaining_requests.push(request);
                                }
                            }
                        }
                        Err(e) => {
                            warn!("Error querying local events: {}", e);
                            remaining_requests.push(request);
                            cached_events_batches.push(Vec::new());
                        }
                    }
                }
                Err(e) => {
                    warn!("Error converting request to filter: {}", e);
                    remaining_requests.push(request);
                    cached_events_batches.push(Vec::new());
                }
            }
        }

        debug!(
            "Found {} cached event batches, {} remaining requests",
            cached_events_batches.len(),
            remaining_requests.len()
        );

        Ok((remaining_requests, cached_events_batches))
    }

    async fn find_event_context(&self, event: &ParsedEvent, _max_depth: usize) -> Vec<ParsedEvent> {
        // Simplified implementation to avoid Send trait issues
        let mut context_events = Vec::new();

        // Process requests from this event (matching Go implementation)
        if let Some(requests) = &event.requests {
            for request in requests {
                if let Ok(filter) = request.to_filter() {
                    if let Ok(related_events) =
                        EventDatabase::query_events(&*self.database, filter).await
                    {
                        context_events.extend(related_events);
                    }
                }
            }
        }

        context_events
    }
}
