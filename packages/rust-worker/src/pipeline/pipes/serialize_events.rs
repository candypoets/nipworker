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
        if let Some(parsed_event) = event.parsed {
            let mut builder = FlatBufferBuilder::new();

            // Get the parsed data and build the ParsedData union using shared builder
            let parsed_data = parsed_event
                .parsed
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("No parsed data available"))?;

            // Build the ParsedData union directly in our builder
            let (parsed_type, parsed_union_offset) = parsed_data.build_flatbuffer(&mut builder)?;
            tracing::debug!("ParsedData union built - type: {:?}", parsed_type);
            tracing::debug!("Parsed union offset created successfully");

            // Build the NostrEvent from parsed_event.event
            let id_offset = builder.create_string(&parsed_event.event.id.to_hex());
            let pubkey_offset = builder.create_string(&parsed_event.event.pubkey.to_hex());

            // Build relays based on source
            let relays_offsets: Vec<_> = if let Some(relay) = event.source_relay {
                vec![builder.create_string(&relay)]
            } else {
                vec![]
            };
            let relays_offset = if relays_offsets.is_empty() {
                None
            } else {
                Some(builder.create_vector(&relays_offsets))
            };

            let requests_offset = if let Some(reqs) = parsed_event.requests.as_ref() {
                if !reqs.is_empty() {
                    let req_offsets: Vec<_> = reqs
                        .iter()
                        .map(|r| r.build_flatbuffer(&mut builder))
                        .collect();
                    Some(builder.create_vector(&req_offsets))
                } else {
                    None
                }
            } else {
                None
            };

            // Build ParsedEvent with the union
            tracing::debug!("Building ParsedEvent with parsed_union_offset");
            let parsed_event_args = fb::ParsedEventArgs {
                id: Some(id_offset),
                pubkey: Some(pubkey_offset),
                created_at: parsed_event.event.created_at.as_u64() as u32,
                kind: parsed_event.event.kind.as_u32() as u16,
                parsed_type,
                parsed: Some(parsed_union_offset),
                requests: requests_offset,
                relays: relays_offset,
            };

            let parsed_event_offset = fb::ParsedEvent::create(&mut builder, &parsed_event_args);
            tracing::debug!("Created ParsedEvent offset successfully");

            // Build root WorkerMessage
            let union_value = parsed_event_offset.as_union_value();

            let message_args = fb::WorkerMessageArgs {
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
        } else {
            // Can't serialize unparsed events
            Ok(PipeOutput::Drop)
        }
    }

    fn can_direct_output(&self) -> bool {
        true // This is designed to be terminal
    }

    fn name(&self) -> &str {
        &self.name
    }
}
