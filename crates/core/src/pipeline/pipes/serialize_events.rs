use super::super::*;
use crate::generated::nostr::fb;
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
        // Reuse bytes stashed by an upstream pipe (e.g. SaveToDbPipe) instead
        // of serializing the same event again. The inner sub_id is ignored
        // downstream (main reads the sub id from the outer tagged framing).
        if let Some(serialized) = event.serialized {
            // Safety check: prevent excessive data
            if serialized.len() > 512 * 1024 {
                warn!(
                    "FlatBuffer data too large: {} bytes for subscription {}",
                    serialized.len(),
                    self.subscription_id
                );
                return Ok(PipeOutput::Drop);
            }
            return Ok(PipeOutput::DirectOutput(serialized));
        }

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

#[cfg(test)]
mod tests {
    use super::*;

    fn event_with_stash(bytes: Vec<u8>) -> PipelineEvent {
        PipelineEvent {
            raw: None,
            parsed: None,
            id: [1; 32],
            source_relay: None,
            serialized: Some(bytes),
        }
    }

    #[tokio::test]
    async fn reuses_stashed_bytes_as_direct_output() {
        let stashed = vec![1u8, 2, 3, 4];
        let mut pipe = SerializeEventsPipe::new("sub".to_string());
        let out = pipe
            .process(event_with_stash(stashed.clone()))
            .await
            .expect("serialize should succeed");
        match out {
            PipeOutput::DirectOutput(bytes) => assert_eq!(bytes, stashed),
            _ => panic!("stashed bytes must be reused as DirectOutput"),
        }
    }

    #[tokio::test]
    async fn stashed_bytes_still_subject_to_size_guard() {
        let oversized = vec![0u8; 512 * 1024 + 1];
        let mut pipe = SerializeEventsPipe::new("sub".to_string());
        let out = pipe
            .process(event_with_stash(oversized))
            .await
            .expect("serialize should succeed");
        assert!(matches!(out, PipeOutput::Drop));
    }

    #[tokio::test]
    async fn drops_when_nothing_to_serialize() {
        let mut pipe = SerializeEventsPipe::new("sub".to_string());
        let event = PipelineEvent {
            raw: None,
            parsed: None,
            id: [1; 32],
            source_relay: None,
            serialized: None,
        };
        let out = pipe.process(event).await.expect("serialize should succeed");
        assert!(matches!(out, PipeOutput::Drop));
    }
}
