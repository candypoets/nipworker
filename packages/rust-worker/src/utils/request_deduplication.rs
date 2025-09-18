//! Request deduplication utility for Nostr event parsers
//!
//! This module provides utilities to deduplicate network requests based on their
//! filter criteria, helping to optimize network usage by merging requests with
//! identical filter parameters while combining their relay lists.

use crate::types::network::Request;
use rustc_hash::FxHashMap;

/// Utility struct for request deduplication operations
pub struct RequestDeduplicator;

impl RequestDeduplicator {
    /// Deduplicate a vector of requests based on their filter criteria
    ///
    /// Requests with identical filter parameters (ids, authors, kinds, tags, etc.)
    /// will be merged into a single request with their relay lists combined.
    ///
    /// # Arguments
    /// * `requests` - Vector of requests to deduplicate
    ///
    /// # Returns
    /// A vector of deduplicated requests with merged relay lists
    pub fn deduplicate_requests(requests: &Vec<Request>) -> Vec<Request> {
        let mut request_map: FxHashMap<String, Request> = FxHashMap::default();

        for request in requests {
            // Create a canonical string representation of the filter criteria
            let filter_key = Self::create_filter_key(&request);

            // If we already have a request with this filter, merge the relays
            if let Some(existing_request) = request_map.get_mut(&filter_key) {
                // Merge relay sets using FxHashSet for deduplication
                let mut relay_set: rustc_hash::FxHashSet<String> =
                    existing_request.relays.iter().cloned().collect();
                for relay in &request.relays {
                    relay_set.insert(relay.clone());
                }
                existing_request.relays = relay_set.into_iter().collect();
                existing_request.relays.sort(); // Sort for consistency
            } else {
                // Create new request with deduplicated relays
                let relay_set: rustc_hash::FxHashSet<String> =
                    request.relays.iter().cloned().collect();
                let new_request = Request {
                    ids: request.ids.clone(),
                    authors: request.authors.clone(),
                    kinds: request.kinds.clone(),
                    tags: request.tags.clone(),
                    since: request.since,
                    until: request.until,
                    limit: request.limit,
                    search: request.search.clone(),
                    close_on_eose: request.close_on_eose,
                    cache_first: request.cache_first,
                    relays: {
                        let mut relays: Vec<String> = relay_set.into_iter().collect();
                        relays.sort();
                        relays
                    },
                };
                request_map.insert(filter_key, new_request);
            }
        }

        request_map.into_values().collect()
    }

    /// Create a canonical string representation of a request's filter criteria
    ///
    /// This function generates a unique string key based on all filter parameters
    /// that affect the actual query behavior. The key is order-independent,
    /// meaning requests with the same filter criteria but different field ordering
    /// will generate the same key.
    ///
    /// # Arguments
    /// * `request` - The request to generate a filter key for
    ///
    /// # Returns
    /// A canonical string representation of the filter criteria
    fn create_filter_key(request: &Request) -> String {
        // Create a canonical string representation of the filter criteria
        let mut key_parts = Vec::new();

        // Add sorted IDs
        if !request.ids.is_empty() {
            let mut sorted_ids = request.ids.clone();
            sorted_ids.sort();
            key_parts.push(format!("ids:{}", sorted_ids.join(",")));
        }

        // Add sorted authors
        if !request.authors.is_empty() {
            let mut sorted_authors = request.authors.clone();
            sorted_authors.sort();
            key_parts.push(format!("authors:{}", sorted_authors.join(",")));
        }

        // Add sorted kinds
        if !request.kinds.is_empty() {
            let mut sorted_kinds = request.kinds.clone();
            sorted_kinds.sort();
            key_parts.push(format!(
                "kinds:{}",
                sorted_kinds
                    .iter()
                    .map(|k| k.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            ));
        }

        // Add sorted tags
        if !request.tags.is_empty() {
            let mut sorted_tags = request.tags.iter().collect::<Vec<_>>();
            sorted_tags.sort_by_key(|(k, _)| *k);
            let tags_str = sorted_tags
                .iter()
                .map(|(k, v)| {
                    let mut sorted_values = (*v).clone();
                    sorted_values.sort();
                    format!("{}:{}", k, sorted_values.join(","))
                })
                .collect::<Vec<_>>()
                .join(";");
            key_parts.push(format!("tags:{}", tags_str));
        }

        // Add temporal constraints
        if let Some(since) = request.since {
            key_parts.push(format!("since:{}", since));
        }
        if let Some(until) = request.until {
            key_parts.push(format!("until:{}", until));
        }

        // Add limit
        if let Some(limit) = request.limit {
            key_parts.push(format!("limit:{}", limit));
        }

        // Add search
        if let Some(search_term) = &request.search {
            key_parts.push(format!("search:{}", search_term));
        }

        // Add other filter-relevant fields
        key_parts.push(format!("close_on_eose:{}", request.close_on_eose));
        key_parts.push(format!("cache_first:{}", request.cache_first));

        key_parts.join("|")
    }
}
