use crate::db::NostrDB;
use crate::network::interfaces::{CacheProcessor as CacheProcessorTrait, EventDatabase};
use crate::parsed_event::ParsedEvent;
use crate::parser::Parser;
use crate::types::network::Request;
use anyhow::Result;
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
}

impl CacheProcessorTrait for CacheProcessor {
    async fn process_local_requests(
        &self,
        requests: Vec<Request>,
        max_depth: usize,
    ) -> Result<(Vec<Request>, Vec<Vec<Vec<u8>>>)> {
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

                                cached_events_batches.push(events);

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

    async fn find_event_context(&self, event: &ParsedEvent, _max_depth: usize) -> Vec<Vec<u8>> {
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
