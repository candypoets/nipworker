use crate::generated::nostr::fb;

use super::super::*;
use flatbuffers::FlatBufferBuilder;
use rustc_hash::{FxHashMap, FxHashSet};
use tracing::error;

pub struct CounterPipe {
    kinds: FxHashSet<u16>,
    counts: FxHashMap<u16, u64>,
    pubkey: String, // Pubkey in hex string
    name: String,
}

impl CounterPipe {
    pub fn new(kinds: Vec<u16>, pubkey: String) -> Self {
        let counts = kinds.iter().map(|&k| (k, 0)).collect();
        Self {
            name: format!("Counter({:?})", kinds),
            kinds: kinds.into_iter().map(|k| k as u16).collect(),
            pubkey,
            counts,
        }
    }
}

impl Pipe for CounterPipe {
    async fn process(&mut self, event: PipelineEvent) -> Result<PipeOutput> {
        // Get kind from either raw or parsed event
        let (kind, pubkey) = if let Some(ref raw) = event.raw {
            (raw.kind, raw.pubkey.to_string())
        } else if let Some(ref parsed) = event.parsed {
            (parsed.event.kind, parsed.event.pubkey.to_string())
        } else {
            return Ok(PipeOutput::Drop);
        };

        if self.kinds.contains(&kind) {
            *self.counts.entry(kind).or_insert(0) += 1;

            let mut fbb = FlatBufferBuilder::new();

            let counter_args = fb::CountResponseArgs {
                count: *self.counts.get(&kind).unwrap_or(&0) as u32,
                kind: kind as u16,
                you: self.pubkey == pubkey,
            };

            let counter_offset = fb::CountResponse::create(&mut fbb, &counter_args);

            let worker_msg = {
                let args = fb::WorkerMessageArgs {
                    type_: fb::MessageType::CountResponse,
                    content_type: fb::Message::CountResponse,
                    content: Some(counter_offset.as_union_value()),
                };
                fb::WorkerMessage::create(&mut fbb, &args)
            };

            fbb.finish(worker_msg, None);

            let flatbuffer_data = fbb.finished_data().to_vec();

            Ok(PipeOutput::DirectOutput(flatbuffer_data))
        } else {
            // Drop all events - we're only counting
            Ok(PipeOutput::Drop)
        }
    }

    async fn process_cached_batch(&mut self, messages: &[Vec<u8>]) -> Result<Vec<Vec<u8>>> {
        let mut outputs = Vec::new();
        let mut you_by_kind: FxHashMap<u16, bool> = FxHashMap::default();

        for bytes in messages {
            if let Ok(msg) = flatbuffers::root::<fb::WorkerMessage>(&bytes) {
                if let fb::Message::ParsedEvent = msg.content_type() {
                    if let Some(parsed) = msg.content_as_parsed_event() {
                        let kind = parsed.kind();
                        let pubkey = parsed.pubkey().to_string();

                        if self.kinds.contains(&kind) {
                            *self.counts.entry(kind).or_insert(0) += 1;

                            // Track if any event for this kind is from "you"
                            if self.pubkey == pubkey {
                                you_by_kind.insert(kind, true);
                            }
                        }
                    }
                }
            }
        }

        for &kind in &self.kinds {
            let mut fbb = FlatBufferBuilder::new();
            let counter_args = fb::CountResponseArgs {
                count: *self.counts.get(&kind).unwrap_or(&0) as u32,
                kind: kind as u16,
                you: *you_by_kind.get(&kind).unwrap_or(&false),
            };
            let counter_offset = fb::CountResponse::create(&mut fbb, &counter_args);

            let worker_msg = {
                let args = fb::WorkerMessageArgs {
                    type_: fb::MessageType::CountResponse,
                    content_type: fb::Message::CountResponse,
                    content: Some(counter_offset.as_union_value()),
                };
                fb::WorkerMessage::create(&mut fbb, &args)
            };

            fbb.finish(worker_msg, None);
            outputs.push(fbb.finished_data().to_vec());
        }

        Ok(outputs)
    }

    fn can_direct_output(&self) -> bool {
        true // Counter can be terminal and send counts directly
    }

    fn name(&self) -> &str {
        &self.name
    }
}
