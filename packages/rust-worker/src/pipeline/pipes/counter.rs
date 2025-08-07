use super::super::*;
use rustc_hash::{FxHashMap, FxHashSet};
use tracing::error;

pub struct CounterPipe {
    kinds: FxHashSet<u64>,
    counts: FxHashMap<u64, u64>,
    pubkey: String, // Pubkey in hex string
    name: String,
}

impl CounterPipe {
    pub fn new(kinds: Vec<u64>, pubkey: String) -> Self {
        let counts = kinds.iter().map(|&k| (k, 0)).collect();
        Self {
            name: format!("Counter({:?})", kinds),
            kinds: kinds.into_iter().collect(),
            pubkey,
            counts,
        }
    }
}

impl Pipe for CounterPipe {
    async fn process(&mut self, event: PipelineEvent) -> Result<PipeOutput> {
        tracing::info!("Processing event in CounterPipe");
        // Get kind from either raw or parsed event
        let (kind, pubkey) = if let Some(ref raw) = event.raw {
            (raw.kind.as_u64(), raw.pubkey.to_string())
        } else if let Some(ref parsed) = event.parsed {
            (parsed.event.kind.as_u64(), parsed.event.pubkey.to_string())
        } else {
            return Ok(PipeOutput::Drop);
        };

        if self.kinds.contains(&kind) {
            *self.counts.entry(kind).or_insert(0) += 1;

            let message = WorkerToMainMessage::Count {
                count: *self.counts.get(&kind).unwrap_or(&0) as u32,
                kind: kind as u32,
                you: pubkey == self.pubkey,
                metadata: String::new(),
            };

            match rmp_serde::to_vec_named(&message) {
                Ok(msgpack) => Ok(PipeOutput::DirectOutput(msgpack)),
                Err(e) => {
                    error!("Failed to serialize counter event: {}", e);
                    Ok(PipeOutput::Drop)
                }
            }
        } else {
            // Drop all events - we're only counting
            Ok(PipeOutput::Drop)
        }
    }

    fn can_direct_output(&self) -> bool {
        true // Counter can be terminal and send counts directly
    }

    fn name(&self) -> &str {
        &self.name
    }
}
