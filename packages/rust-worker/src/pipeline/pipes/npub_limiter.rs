use super::super::*;
use rustc_hash::FxHashMap;
use std::collections::VecDeque;

struct NpubTracker {
    last_forwarded_timestamp: Option<u64>,
    forwarded_count: usize,
}

pub struct NpubLimiterPipe {
    kind: u64,
    limit_per_npub: usize,
    max_total_npubs: usize,
    npub_trackers: FxHashMap<String, NpubTracker>,
    name: String,
}

impl NpubLimiterPipe {
    pub fn new(kind: u64, limit_per_npub: usize, max_total_npubs: usize) -> Self {
        Self {
            name: format!("NpubLimiter(kind:{}, limit:{})", kind, limit_per_npub),
            kind,
            limit_per_npub,
            max_total_npubs,
            npub_trackers: FxHashMap::default(),
        }
    }
}

impl Pipe for NpubLimiterPipe {
    async fn process(&mut self, event: PipelineEvent) -> Result<PipeOutput> {
        // Get the nostr event from either raw or parsed
        let nostr_event = if let Some(ref raw) = event.raw {
            raw
        } else if let Some(ref parsed) = event.parsed {
            &parsed.event
        } else {
            return Ok(PipeOutput::Drop);
        };

        // Get kind, pubkey, and timestamp from the nostr event
        let kind = nostr_event.kind.as_u64();
        let pubkey = nostr_event.pubkey.to_string();
        let created_at = nostr_event.created_at.as_u64();

        // Only process events of the specified kind
        if kind != self.kind {
            return Ok(PipeOutput::Drop);
        }

        // Prevent memory explosion by limiting total tracked npubs
        // if self.npub_trackers.len() > self.max_total_npubs {
        //     // Remove oldest npub (simple cleanup - could be improved with LRU)
        //     if let Some(oldest_key) = self.npub_trackers.keys().next().cloned() {
        //         self.npub_trackers.remove(&oldest_key);
        //     }
        // }

        // Get or create npub tracker
        let tracker = self
            .npub_trackers
            .entry(pubkey.clone())
            .or_insert_with(|| NpubTracker {
                last_forwarded_timestamp: None,
                forwarded_count: 0,
            });

        // If we haven't forwarded any events yet, always forward this one
        if tracker.last_forwarded_timestamp.is_none() {
            tracker.last_forwarded_timestamp = Some(created_at);
            tracker.forwarded_count = 1;
            return Ok(PipeOutput::Event(event));
        }

        let last_timestamp = tracker.last_forwarded_timestamp.unwrap();

        // If this event is newer than the last forwarded, update tracker and forward
        if created_at > last_timestamp {
            tracker.forwarded_count = if tracker.forwarded_count >= self.limit_per_npub {
                self.limit_per_npub
            } else {
                tracker.forwarded_count + 1
            };
            Ok(PipeOutput::Event(event))
        } else if tracker.forwarded_count < self.limit_per_npub {
            // Event is older but we haven't reached the limit yet
            tracker.forwarded_count += 1;
            tracker.last_forwarded_timestamp = Some(created_at);
            Ok(PipeOutput::Event(event))
        } else {
            Ok(PipeOutput::Drop)
        }
    }

    fn name(&self) -> &str {
        &self.name
    }
}
