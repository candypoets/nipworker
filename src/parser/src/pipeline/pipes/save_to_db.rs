use super::super::*;
use flatbuffers::FlatBufferBuilder;
use shared::{generated::nostr::fb, Port};
use std::cell::RefCell;
use std::rc::Rc;
use tracing::{info, warn};

pub struct SaveToDbPipe {
    to_cache: Rc<RefCell<Port>>,
    name: String,
}

impl SaveToDbPipe {
    pub fn new(to_cache: Rc<RefCell<Port>>) -> Self {
        Self {
            name: "SaveToDb".to_string(),
            to_cache,
        }
    }

    /// Send event as WorkerMessage to cache.
    /// If parsed, send as ParsedNostrEvent. Otherwise as NostrEvent.
    fn send_to_cache(&self, event: &PipelineEvent) {
        let mut builder = FlatBufferBuilder::new();
        let sub_id_offset = builder.create_string("save_to_db");

        // Determine what to send based on what we have
        let (msg_type, content_type, content_offset) = if let Some(ref parsed) = event.parsed {
            // Send as ParsedEvent (includes decrypted content for kind4!)
            info!("SaveToDb: Sending parsed event kind={} to cache", parsed.event.kind);
            match parsed.build_flatbuffer(&mut builder) {
                Ok(offset) => (
                    fb::MessageType::ParsedNostrEvent,
                    fb::Message::ParsedEvent,
                    offset.as_union_value(),
                ),
                Err(e) => {
                    warn!("Failed to build ParsedEvent flatbuffer: {}", e);
                    return;
                }
            }
        } else if let Some(ref raw) = event.raw {
            // Send as NostrEvent
            let offset = raw.build_flatbuffer(&mut builder);
            (
                fb::MessageType::NostrEvent,
                fb::Message::NostrEvent,
                offset.as_union_value(),
            )
        } else {
            // Nothing to send
            return;
        };

        let worker_msg = fb::WorkerMessage::create(
            &mut builder,
            &fb::WorkerMessageArgs {
                sub_id: Some(sub_id_offset),
                url: None,
                type_: msg_type,
                content_type,
                content: Some(content_offset),
            },
        );

        builder.finish(worker_msg, None);
        let _ = self.to_cache.borrow().send(builder.finished_data());
    }
}

impl Pipe for SaveToDbPipe {
    async fn process(&mut self, event: PipelineEvent) -> Result<PipeOutput> {
        // Send event as WorkerMessage (ParsedEvent if available, else NostrEvent)
        self.send_to_cache(&event);

        // Always pass the event through unchanged
        Ok(PipeOutput::Event(event))
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn run_for_cached_events(&self) -> bool {
        return false;
    }
}
