use super::super::*;
use std::collections::{HashMap, VecDeque};

struct NpubEvents {
    events: VecDeque<PipelineEvent>,
}

pub struct NpubLimiterPipe {
    kind: u64,
    limit_per_npub: usize,
    max_total_npubs: usize,
    npub_events: HashMap<String, NpubEvents>,
    name: String,
}

impl NpubLimiterPipe {
    pub fn new(kind: u64, limit_per_npub: usize, max_total_npubs: usize) -> Self {
        Self {
            name: format!("NpubLimiter(kind:{}, limit:{})", kind, limit_per_npub),
            kind,
            limit_per_npub,
            max_total_npubs,
            npub_events: HashMap::new(),
        }
    }
}

#[async_trait(?Send)]
impl Pipe for NpubLimiterPipe {
    async fn process(&mut self, event: PipelineEvent) -> Result<PipeOutput> {
        // Get kind and pubkey from either raw or parsed event
        let (kind, pubkey) = if let Some(ref raw) = event.raw {
            (raw.kind.as_u64(), raw.pubkey.to_string())
        } else if let Some(ref parsed) = event.parsed {
            (parsed.event.kind.as_u64(), parsed.event.pubkey.to_string())
        } else {
            return Ok(PipeOutput::Drop);
        };

        // Only process events of the specified kind
        if kind != self.kind {
            return Ok(PipeOutput::Drop);
        }

        // Prevent memory explosion by limiting total tracked npubs
        if self.npub_events.len() > self.max_total_npubs {
            // Remove oldest npub (simple cleanup - could be improved with LRU)
            if let Some(oldest_key) = self.npub_events.keys().next().cloned() {
                self.npub_events.remove(&oldest_key);
            }
        }

        // Get or create npub events tracker
        let npub_events = self
            .npub_events
            .entry(pubkey.clone())
            .or_insert_with(|| NpubEvents {
                events: VecDeque::new(),
            });

        // Add new event and maintain limit
        npub_events.events.push_back(event);
        if npub_events.events.len() > self.limit_per_npub {
            npub_events.events.pop_front();
        }

        // Forward the current event (it's now stored and limited)
        if let Some(latest_event) = npub_events.events.back() {
            Ok(PipeOutput::Event(latest_event.clone()))
        } else {
            Ok(PipeOutput::Drop)
        }
    }

    fn name(&self) -> &str {
        &self.name
    }
}
