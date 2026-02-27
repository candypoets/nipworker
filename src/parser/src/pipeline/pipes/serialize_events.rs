use super::super::*;
use shared::generated::nostr::fb;
use flatbuffers::FlatBufferBuilder;
use tracing::warn;

/// Terminal pipe that serializes events using WorkerToMainMessage format
pub struct SerializeEventsPipe {
    subscription_id: String,
    name: String,
}

impl SerializeEventsPipe {
    pub fn new(subscription_id: String) -> Self {
        Self {
            name: format!("SerializeEvents({})", subscription_id),
            subscription_id,
        }
    }
}

impl Pipe for SerializeEventsPipe {
    async fn process(&mut self, event: PipelineEvent) -> Result<PipeOutput> {
        if let Some(parsed_event) = event.parsed {
            let mut builder = FlatBufferBuilder::new();

            let parsed_event_offset = parsed_event.build_flatbuffer(&mut builder)?;

            // Build root WorkerMessage
            let union_value = parsed_event_offset.as_union_value();

            let message_args = fb::WorkerMessageArgs {
                sub_id: None,
                url: None,
                type_: fb::MessageType::ParsedNostrEvent,
                content_type: fb::Message::ParsedEvent,
                content: Some(union_value),
            };

            let root = fb::WorkerMessage::create(&mut builder, &message_args);
            builder.finish(root, None);

            let flatbuffer_data = builder.finished_data().to_vec();

            // Safety check: prevent excessive data
            if flatbuffer_data.len() > 512 * 1024 {
                warn!(
                    "FlatBuffer data too large: {} bytes for subscription {}",
                    flatbuffer_data.len(),
                    self.subscription_id
                );
                return Ok(PipeOutput::Drop);
            }

            Ok(PipeOutput::DirectOutput(flatbuffer_data))
        } else if let Some(ref nostr_event) = event.raw {
            let mut builder = FlatBufferBuilder::new();

            // Build NostrEvent table
            let nostr_event_offset = nostr_event.build_flatbuffer(&mut builder);
            let union_value = nostr_event_offset.as_union_value();

            // Wrap into WorkerMessage as a NostrEvent
            let message_args = fb::WorkerMessageArgs {
                sub_id: None,
                url: None,
                type_: fb::MessageType::NostrEvent,
                content_type: fb::Message::NostrEvent,
                content: Some(union_value),
            };

            let root = fb::WorkerMessage::create(&mut builder, &message_args);
            builder.finish(root, None);

            let flatbuffer_data = builder.finished_data().to_vec();

            // Safety check: prevent excessive data
            if flatbuffer_data.len() > 512 * 1024 {
                warn!(
                    "FlatBuffer data too large: {} bytes for subscription {}",
                    flatbuffer_data.len(),
                    self.subscription_id
                );
                return Ok(PipeOutput::Drop);
            }

            Ok(PipeOutput::DirectOutput(flatbuffer_data))
        } else {
            // Can't serialize if neither parsed nor raw is available
            Ok(PipeOutput::Drop)
        }
    }

    async fn process_cached_batch(&mut self, messages: &[Vec<u8>]) -> Result<Vec<Vec<u8>>> {
        // Cached events are already WorkerMessage bytes from the cache.
        // Just pass them through - no need to re-serialize.
        // The frontend expects WorkerMessage format via the batch buffer.
        Ok(messages.to_vec())
    }

    fn can_direct_output(&self) -> bool {
        true // This is designed to be terminal
    }

    fn name(&self) -> &str {
        &self.name
    }
}
