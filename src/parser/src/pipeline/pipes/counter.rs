use shared::generated::nostr::fb;

use super::super::*;
use flatbuffers::FlatBufferBuilder;
use rustc_hash::{FxHashMap, FxHashSet};

pub struct CounterPipe {
    kinds: FxHashSet<u16>,
    counts: FxHashMap<u16, u64>,
    you_by_kind: FxHashMap<u16, bool>,
    pubkey: String,
    name: String,
    eosed: bool,
}

impl CounterPipe {
    pub fn new(kinds: Vec<u16>, pubkey: String) -> Self {
        let counts = kinds.iter().map(|&k| (k, 0)).collect();
        Self {
            name: format!("Counter({:?})", kinds),
            kinds: kinds.into_iter().map(|k| k as u16).collect(),
            counts,
            you_by_kind: FxHashMap::default(),
            pubkey,
            eosed: false,
        }
    }

    /// Build a CountResponse for a specific kind
    fn build_count_response(&self, kind: u16) -> Vec<u8> {
        let mut fbb = FlatBufferBuilder::new();

        let counter_args = fb::CountResponseArgs {
            count: *self.counts.get(&kind).unwrap_or(&0) as u32,
            kind: kind as u16,
            you: *self.you_by_kind.get(&kind).unwrap_or(&false),
        };

        let counter_offset = fb::CountResponse::create(&mut fbb, &counter_args);

        let worker_msg = fb::WorkerMessage::create(
            &mut fbb,
            &fb::WorkerMessageArgs {
                sub_id: None,
                url: None,
                type_: fb::MessageType::CountResponse,
                content_type: fb::Message::CountResponse,
                content: Some(counter_offset.as_union_value()),
            },
        );

        fbb.finish(worker_msg, None);
        fbb.finished_data().to_vec()
    }

    /// Build CountResponses for all tracked kinds
    fn build_all_count_responses(&self) -> Vec<Vec<u8>> {
        self.kinds
            .iter()
            .map(|&k| self.build_count_response(k))
            .collect()
    }
}

impl Pipe for CounterPipe {
    async fn process(&mut self, event: PipelineEvent) -> Result<PipeOutput> {
        let (kind, pubkey) = if let Some(ref raw) = event.raw {
            (raw.kind, raw.pubkey.to_string())
        } else if let Some(ref parsed) = event.parsed {
            (parsed.event.kind, parsed.event.pubkey.to_string())
        } else {
            return Ok(PipeOutput::Drop);
        };

        if !self.kinds.contains(&kind) {
            return Ok(PipeOutput::Drop);
        }

        // Accumulate count
        *self.counts.entry(kind).or_insert(0) += 1;

        // Track "you" status
        if self.pubkey == pubkey {
            self.you_by_kind.insert(kind, true);
        }

        // After EOSE: emit immediately (only the changed kind)
        if self.eosed {
            return Ok(PipeOutput::DirectOutput(self.build_count_response(kind)));
        }

        // Before EOSE: accumulate without emitting
        Ok(PipeOutput::Drop)
    }

    async fn process_cached_batch(&mut self, messages: &[Vec<u8>]) -> Result<Vec<Vec<u8>>> {
        // Accumulate without emitting
        for bytes in messages {
            if let Ok(msg) = flatbuffers::root::<fb::WorkerMessage>(&bytes) {
                if let fb::Message::ParsedEvent = msg.content_type() {
                    if let Some(parsed) = msg.content_as_parsed_event() {
                        let kind = parsed.kind();
                        let pubkey = parsed.pubkey().to_string();

                        if self.kinds.contains(&kind) {
                            *self.counts.entry(kind).or_insert(0) += 1;
                            if self.pubkey == pubkey {
                                self.you_by_kind.insert(kind, true);
                            }
                        }
                    }
                }
            }
        }
        Ok(Vec::new())
    }

    fn flush(&mut self) -> Vec<Vec<u8>> {
        // Emit all kinds at EOSE/EOCE
        self.build_all_count_responses()
    }

    fn on_eose(&mut self) {
        // After this, process() will emit immediately
        self.eosed = true;
    }

    fn can_direct_output(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        &self.name
    }
}
