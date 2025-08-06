use super::super::*;
use crate::types::network::SubscribeKind;
use crate::types::SerializableParsedEvent;
use crate::WorkerToMainMessage;
use tracing::{error, warn};

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
            // Convert ParsedEvent to SerializableParsedEvent to ensure id and sig fields are hex strings
            let serializable_event = SerializableParsedEvent::from(parsed_event);

            // Determine event type based on source
            let event_type = if event.source_relay.is_some() {
                SubscribeKind::FetchedEvent
            } else {
                SubscribeKind::CachedEvent
            };

            let message = WorkerToMainMessage::SubscriptionEvent {
                subscription_id: self.subscription_id.clone(),
                event_type,
                event_data: vec![vec![serializable_event]],
            };

            match rmp_serde::to_vec_named(&message) {
                Ok(msgpack) => {
                    // Safety check: prevent excessive serialized data
                    if msgpack.len() > 512 * 1024 {
                        // 512KB limit
                        warn!(
                            "Serialized data too large: {} bytes for subscription {}",
                            msgpack.len(),
                            self.subscription_id
                        );
                        return Ok(PipeOutput::Drop);
                    }
                    Ok(PipeOutput::DirectOutput(msgpack))
                }
                Err(e) => {
                    error!(
                        "Failed to serialize event for subscription {}: {}",
                        self.subscription_id, e
                    );
                    Ok(PipeOutput::Drop)
                }
            }
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
