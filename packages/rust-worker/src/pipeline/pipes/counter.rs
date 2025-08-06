use super::super::*;
use rustc_hash::{FxHashMap, FxHashSet};
use serde_json::json;

pub struct CounterPipe {
    kinds: FxHashSet<u64>,
    counts: FxHashMap<u64, u64>,
    update_interval: u64,
    total_processed: u64,
    name: String,
}

impl CounterPipe {
    pub fn new(kinds: Vec<u64>, update_interval: u64) -> Self {
        let counts = kinds.iter().map(|&k| (k, 0)).collect();
        Self {
            name: format!("Counter({:?})", kinds),
            kinds: kinds.into_iter().collect(),
            counts,
            update_interval,
            total_processed: 0,
        }
    }
}

impl Pipe for CounterPipe {
    async fn process(&mut self, event: PipelineEvent) -> Result<PipeOutput> {
        // Get kind from either raw or parsed event
        let kind = if let Some(ref raw) = event.raw {
            raw.kind.as_u64()
        } else if let Some(ref parsed) = event.parsed {
            parsed.event.kind.as_u64()
        } else {
            return Ok(PipeOutput::Drop);
        };

        if self.kinds.contains(&kind) {
            *self.counts.entry(kind).or_insert(0) += 1;
            self.total_processed += 1;

            if self.total_processed % self.update_interval == 0 {
                let counts_json = json!({
                    "type": "kind_counts",
                    "counts": self.counts,
                    "total": self.total_processed
                });
                let data = serde_json::to_vec(&counts_json)?;
                return Ok(PipeOutput::DirectOutput(data));
            }
        }

        // Drop all events - we're only counting
        Ok(PipeOutput::Drop)
    }

    fn can_direct_output(&self) -> bool {
        true // Counter can be terminal and send counts directly
    }

    fn name(&self) -> &str {
        &self.name
    }
}
