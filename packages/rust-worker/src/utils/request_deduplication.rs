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
    pub fn deduplicate_requests(requests: Vec<Request>) -> Vec<Request> {
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
                let mut new_request = request.clone();
                new_request.relays = relay_set.into_iter().collect();
                new_request.relays.sort(); // Sort for consistency
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
        if !request.search.is_empty() {
            key_parts.push(format!("search:{}", request.search));
        }

        // Add other filter-relevant fields
        key_parts.push(format!("close_on_eose:{}", request.close_on_eose));
        key_parts.push(format!("cache_first:{}", request.cache_first));
        key_parts.push(format!("no_optimize:{}", request.no_optimize));
        key_parts.push(format!("count:{}", request.count));
        key_parts.push(format!("no_context:{}", request.no_context));

        key_parts.join("|")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_request_deduplication() {
        // Create multiple requests with the same filter criteria but different relays
        let requests = vec![
            Request {
                ids: vec!["event1".to_string(), "event2".to_string()],
                authors: vec!["author1".to_string()],
                kinds: vec![1, 6],
                relays: vec!["relay1.com".to_string(), "relay2.com".to_string()],
                limit: Some(1),
                close_on_eose: true,
                cache_first: true,
                ..Default::default()
            },
            Request {
                ids: vec!["event2".to_string(), "event1".to_string()], // Same IDs, different order
                authors: vec!["author1".to_string()],
                kinds: vec![6, 1], // Same kinds, different order
                relays: vec!["relay2.com".to_string(), "relay3.com".to_string()],
                limit: Some(1),
                close_on_eose: true,
                cache_first: true,
                ..Default::default()
            },
            Request {
                ids: vec!["event3".to_string()],
                authors: vec!["author2".to_string()],
                kinds: vec![1],
                relays: vec!["relay1.com".to_string()],
                limit: Some(1),
                close_on_eose: true,
                cache_first: true,
                ..Default::default()
            },
            Request {
                ids: vec!["event1".to_string(), "event2".to_string()],
                authors: vec!["author1".to_string()],
                kinds: vec![1, 6],
                relays: vec!["relay4.com".to_string(), "relay1.com".to_string()],
                limit: Some(2), // Different limit - should NOT be deduplicated
                close_on_eose: true,
                cache_first: true,
                ..Default::default()
            },
        ];

        let deduplicated = RequestDeduplicator::deduplicate_requests(requests);

        // Should have 3 unique requests (2 with same filter but different limits, 1 unique)
        assert_eq!(deduplicated.len(), 3);

        // Find requests with event1 and event2
        let matching_requests: Vec<_> = deduplicated
            .iter()
            .filter(|r| {
                r.ids.contains(&"event1".to_string()) && r.ids.contains(&"event2".to_string())
            })
            .collect();

        // Should have 2 requests with event1+event2 (different limits)
        assert_eq!(matching_requests.len(), 2);

        // Find the request with limit 1
        let limit_1_request = matching_requests
            .iter()
            .find(|r| r.limit == Some(1))
            .unwrap();

        // Should have 3 relays deduplicated
        assert_eq!(limit_1_request.relays.len(), 3);
        assert!(limit_1_request.relays.contains(&"relay1.com".to_string()));
        assert!(limit_1_request.relays.contains(&"relay2.com".to_string()));
        assert!(limit_1_request.relays.contains(&"relay3.com".to_string()));

        // Find the request with limit 2
        let limit_2_request = matching_requests
            .iter()
            .find(|r| r.limit == Some(2))
            .unwrap();

        // Should have 2 relays
        assert_eq!(limit_2_request.relays.len(), 2);
        assert!(limit_2_request.relays.contains(&"relay1.com".to_string()));
        assert!(limit_2_request.relays.contains(&"relay4.com".to_string()));

        // Find the request with event3
        let single_event_request = deduplicated
            .iter()
            .find(|r| r.ids.contains(&"event3".to_string()))
            .unwrap();

        // Should have only one relay
        assert_eq!(single_event_request.relays.len(), 1);
        assert!(single_event_request
            .relays
            .contains(&"relay1.com".to_string()));
    }

    #[test]
    fn test_filter_key_generation() {
        let request = Request {
            ids: vec!["event2".to_string(), "event1".to_string()],
            authors: vec!["author1".to_string()],
            kinds: vec![6, 1],
            limit: Some(10),
            close_on_eose: true,
            cache_first: false,
            ..Default::default()
        };

        let key = RequestDeduplicator::create_filter_key(&request);

        // Key should be order-independent
        assert!(key.contains("ids:event1,event2"));
        assert!(key.contains("authors:author1"));
        assert!(key.contains("kinds:1,6"));
        assert!(key.contains("limit:10"));
        assert!(key.contains("close_on_eose:true"));
        assert!(key.contains("cache_first:false"));
    }

    #[test]
    fn test_tag_deduplication() {
        let mut tags1 = FxHashMap::default();
        tags1.insert(
            "#t".to_string(),
            vec!["bitcoin".to_string(), "nostr".to_string()],
        );

        let mut tags2 = FxHashMap::default();
        tags2.insert(
            "#t".to_string(),
            vec!["nostr".to_string(), "bitcoin".to_string()],
        ); // Same tags, different order

        let request1 = Request {
            ids: vec!["event1".to_string()],
            tags: tags1,
            ..Default::default()
        };

        let request2 = Request {
            ids: vec!["event1".to_string()],
            tags: tags2,
            ..Default::default()
        };

        let key1 = RequestDeduplicator::create_filter_key(&request1);
        let key2 = RequestDeduplicator::create_filter_key(&request2);

        // Keys should be identical despite different tag ordering
        assert_eq!(key1, key2);
    }

    #[test]
    fn test_empty_request_deduplication() {
        let requests = vec![
            Request::default(),
            Request::default(),
            Request {
                relays: vec!["relay1.com".to_string()],
                ..Default::default()
            },
        ];

        let deduplicated = RequestDeduplicator::deduplicate_requests(requests);

        // Should have 1 request with merged relays
        assert_eq!(deduplicated.len(), 1);
        assert_eq!(deduplicated[0].relays.len(), 1);
        assert!(deduplicated[0].relays.contains(&"relay1.com".to_string()));
    }
}
