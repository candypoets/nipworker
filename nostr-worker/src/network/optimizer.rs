use crate::network::interfaces::SubscriptionOptimizer as SubscriptionOptimizerTrait;
use crate::types::network::Request;
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use tracing::{debug, info};

pub struct SubscriptionOptimizer {
    max_authors_per_request: usize,
    max_ids_per_request: usize,
    max_kinds_per_request: usize,
}

impl SubscriptionOptimizer {
    pub fn new() -> Self {
        Self {
            max_authors_per_request: 100,
            max_ids_per_request: 100,
            max_kinds_per_request: 20,
        }
    }

    fn group_requests_by_relay(&self, requests: Vec<Request>) -> HashMap<String, Vec<Request>> {
        let mut relay_groups: HashMap<String, Vec<Request>> = HashMap::new();

        for request in requests {
            for relay in &request.relays {
                relay_groups
                    .entry(relay.clone())
                    .or_insert_with(Vec::new)
                    .push(request.clone());
            }
        }

        relay_groups
    }

    fn merge_compatible_requests(&self, requests: Vec<Request>) -> Vec<Request> {
        if requests.len() <= 1 {
            return requests;
        }

        let mut merged = Vec::new();
        let mut processed = HashSet::new();

        for i in 0..requests.len() {
            if processed.contains(&i) {
                continue;
            }

            let mut base_request = requests[i].clone();
            processed.insert(i);

            // Try to merge with other compatible requests
            for j in (i + 1)..requests.len() {
                if processed.contains(&j) {
                    continue;
                }

                let other_request = &requests[j];

                if self.are_requests_mergeable(&base_request, other_request) {
                    base_request = self.merge_two_requests(base_request, other_request.clone());
                    processed.insert(j);
                }
            }

            // Split request if it's too large
            let split_requests = self.split_oversized_request(base_request);
            merged.extend(split_requests);
        }

        merged
    }

    fn are_requests_mergeable(&self, req1: &Request, req2: &Request) -> bool {
        // Check if basic parameters are compatible
        if req1.since != req2.since || req1.until != req2.until {
            return false;
        }

        if req1.close_on_eose != req2.close_on_eose {
            return false;
        }

        if req1.cache_first != req2.cache_first {
            return false;
        }

        if req1.no_context != req2.no_context {
            return false;
        }

        if req1.count != req2.count {
            return false;
        }

        if !req1.search.is_empty() || !req2.search.is_empty() {
            return req1.search == req2.search;
        }

        // Check if merging would exceed limits
        let total_authors = req1.authors.len() + req2.authors.len();
        let total_ids = req1.ids.len() + req2.ids.len();
        let total_kinds = req1.kinds.len() + req2.kinds.len();

        if total_authors > self.max_authors_per_request
            || total_ids > self.max_ids_per_request
            || total_kinds > self.max_kinds_per_request
        {
            return false;
        }

        true
    }

    fn merge_two_requests(&self, mut req1: Request, req2: Request) -> Request {
        // Merge authors
        for author in req2.authors {
            if !req1.authors.contains(&author) {
                req1.authors.push(author);
            }
        }

        // Merge IDs
        for id in req2.ids {
            if !req1.ids.contains(&id) {
                req1.ids.push(id);
            }
        }

        // Merge kinds
        for kind in req2.kinds {
            if !req1.kinds.contains(&kind) {
                req1.kinds.push(kind);
            }
        }

        // Merge tags
        for (key, values) in req2.tags {
            let entry = req1.tags.entry(key).or_insert_with(Vec::new);
            for value in values {
                if !entry.contains(&value) {
                    entry.push(value);
                }
            }
        }

        // Merge relays
        for relay in req2.relays {
            if !req1.relays.contains(&relay) {
                req1.relays.push(relay);
            }
        }

        // Take the larger limit
        if let Some(limit2) = req2.limit {
            req1.limit = Some(req1.limit.map_or(limit2, |limit1| limit1.max(limit2)));
        }

        req1
    }

    fn split_oversized_request(&self, request: Request) -> Vec<Request> {
        let mut split_requests = Vec::new();

        // Check if authors need to be split
        if request.authors.len() > self.max_authors_per_request {
            for chunk in request.authors.chunks(self.max_authors_per_request) {
                let mut new_request = request.clone();
                new_request.authors = chunk.to_vec();
                new_request.ids.clear(); // Clear other fields when splitting by authors
                new_request.kinds.clear();
                split_requests.push(new_request);
            }
            return split_requests;
        }

        // Check if IDs need to be split
        if request.ids.len() > self.max_ids_per_request {
            for chunk in request.ids.chunks(self.max_ids_per_request) {
                let mut new_request = request.clone();
                new_request.ids = chunk.to_vec();
                new_request.authors.clear(); // Clear other fields when splitting by IDs
                new_request.kinds.clear();
                split_requests.push(new_request);
            }
            return split_requests;
        }

        // Check if kinds need to be split
        if request.kinds.len() > self.max_kinds_per_request {
            for chunk in request.kinds.chunks(self.max_kinds_per_request) {
                let mut new_request = request.clone();
                new_request.kinds = chunk.to_vec();
                split_requests.push(new_request);
            }
            return split_requests;
        }

        // No splitting needed
        split_requests.push(request);
        split_requests
    }

    fn deduplicate_requests(&self, requests: Vec<Request>) -> Vec<Request> {
        let mut unique_requests = Vec::new();
        let mut seen_signatures = HashSet::new();

        for request in requests {
            let signature = self.compute_request_signature(&request);
            if !seen_signatures.contains(&signature) {
                seen_signatures.insert(signature);
                unique_requests.push(request);
            }
        }

        unique_requests
    }

    fn compute_request_signature(&self, request: &Request) -> String {
        use std::collections::BTreeMap;

        // Create a normalized representation of the request for deduplication
        let mut authors = request.authors.clone();
        authors.sort();

        let mut ids = request.ids.clone();
        ids.sort();

        let mut kinds = request.kinds.clone();
        kinds.sort();

        let mut relays = request.relays.clone();
        relays.sort();

        let mut tags = BTreeMap::new();
        for (key, values) in &request.tags {
            let mut sorted_values = values.clone();
            sorted_values.sort();
            tags.insert(key, sorted_values);
        }

        format!(
            "{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}",
            authors.join(","),
            ids.join(","),
            kinds
                .iter()
                .map(|k| k.to_string())
                .collect::<Vec<_>>()
                .join(","),
            serde_json::to_string(&tags).unwrap_or_default(),
            request.since.unwrap_or(0),
            request.until.unwrap_or(0),
            request.limit.unwrap_or(0),
            request.search,
            relays.join(","),
            request.close_on_eose,
            request.cache_first,
            request.no_context
        )
    }
}

#[async_trait]
impl SubscriptionOptimizerTrait for SubscriptionOptimizer {
    fn optimize_subscriptions(&self, requests: Vec<Request>) -> Vec<Request> {
        let request_count = requests.len();
        debug!("Optimizing {} subscription requests", request_count);

        if requests.is_empty() {
            return requests;
        }

        // Group requests by relay
        let relay_groups = self.group_requests_by_relay(requests);
        let mut optimized_requests = Vec::new();

        // Optimize each relay group separately
        for (relay, relay_requests) in relay_groups {
            debug!(
                "Optimizing {} requests for relay: {}",
                relay_requests.len(),
                relay
            );

            // Merge compatible requests
            let merged_requests = self.merge_compatible_requests(relay_requests);

            // Set the relay for each merged request
            let mut relay_optimized: Vec<Request> = merged_requests
                .into_iter()
                .map(|mut req| {
                    req.relays = vec![relay.clone()];
                    req
                })
                .collect();

            optimized_requests.append(&mut relay_optimized);
        }

        // Remove duplicate requests
        let deduplicated = self.deduplicate_requests(optimized_requests);

        info!(
            "Optimized subscription requests: {} -> {}",
            request_count,
            deduplicated.len()
        );

        deduplicated
    }
}

impl Default for SubscriptionOptimizer {
    fn default() -> Self {
        Self::new()
    }
}
