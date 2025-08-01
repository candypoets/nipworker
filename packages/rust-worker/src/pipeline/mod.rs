use crate::db::NostrDB;
use crate::parser::Parser;
use crate::types::*;
use anyhow::Result;
use async_trait::async_trait;
use nostr::Event as NostrEvent;
use std::sync::Arc;

pub mod pipes;

pub use pipes::*;

/// Universal event container that flows through the pipeline
#[derive(Debug, Clone)]
pub struct PipelineEvent {
    /// Raw nostr event (if available)
    pub raw: Option<NostrEvent>,
    /// Parsed event (if already parsed)
    pub parsed: Option<ParsedEvent>,
    /// Event ID (always available for deduplication)
    pub id: [u8; 32],
    /// Relay source (if from network)
    pub source_relay: Option<String>,
}

impl PipelineEvent {
    pub fn from_raw(event: NostrEvent, source_relay: Option<String>) -> Self {
        Self {
            id: event.id.to_bytes(),
            raw: Some(event),
            parsed: None,
            source_relay,
        }
    }

    pub fn from_parsed(event: ParsedEvent) -> Self {
        Self {
            id: event.event.id.to_bytes(),
            raw: None,
            parsed: Some(event),
            source_relay: None,
        }
    }

    pub fn is_parsed(&self) -> bool {
        self.parsed.is_some()
    }
}

/// Output from a pipe - single event processing only
#[derive(Debug)]
pub enum PipeOutput {
    /// Continue with this event
    Event(PipelineEvent),
    /// Drop the event
    Drop,
    /// Send custom data directly to buffer
    DirectOutput(Vec<u8>),
}

/// A pipe in the processing pipeline
#[async_trait(?Send)]
pub trait Pipe {
    async fn process(&mut self, event: PipelineEvent) -> Result<PipeOutput>;
    fn name(&self) -> &str;

    /// Whether this pipe can produce DirectOutput (only last pipe should)
    fn can_direct_output(&self) -> bool {
        false
    }
}

/// Pipeline processor
pub struct Pipeline {
    pipes: Vec<Box<dyn Pipe>>,
    subscription_id: String,
}

impl Pipeline {
    pub fn new(pipes: Vec<Box<dyn Pipe>>, subscription_id: String) -> Result<Self> {
        // Validate pipeline: only last pipe can direct output
        for (i, pipe) in pipes.iter().enumerate() {
            let is_last = i == pipes.len() - 1;
            if pipe.can_direct_output() && !is_last {
                return Err(anyhow::anyhow!(
                    "Pipe '{}' can produce DirectOutput but is not the last pipe in pipeline",
                    pipe.name()
                ));
            }
        }

        Ok(Self {
            pipes,
            subscription_id,
        })
    }

    /// Create default pipeline: deduplication + parsing + save to db + serialize events
    pub fn default(
        parser: Arc<Parser>,
        database: Arc<NostrDB>,
        subscription_id: String,
    ) -> Result<Self> {
        Self::new(
            vec![
                Box::new(DeduplicationPipe::new(10000)),
                Box::new(ParsePipe::new(parser)),
                Box::new(SaveToDbPipe::new(database)),
                Box::new(SerializeEventsPipe::new(subscription_id.clone())),
            ],
            subscription_id,
        )
    }

    /// Create proof verification pipeline: deduplication + parsing + proof verification
    pub fn proof_verification(
        parser: Arc<Parser>,
        subscription_id: String,
        max_proofs: usize,
    ) -> Result<Self> {
        Self::new(
            vec![
                Box::new(DeduplicationPipe::new(10000)),
                Box::new(KindFilterPipe::new(vec![9321, 7375])), // Only process cashu events
                Box::new(ParsePipe::new(parser)),
                Box::new(ProofVerificationPipe::new(max_proofs)),
            ],
            subscription_id,
        )
    }

    /// Process a single event through the pipeline
    pub async fn process(&mut self, mut event: PipelineEvent) -> Result<Option<Vec<u8>>> {
        // Flow through each pipe
        let pipes_len = self.pipes.len();
        for (i, pipe) in self.pipes.iter_mut().enumerate() {
            let is_last = i == pipes_len - 1;

            match pipe.process(event).await? {
                PipeOutput::Event(e) => event = e,
                PipeOutput::Drop => return Ok(None),
                PipeOutput::DirectOutput(data) => {
                    if !is_last {
                        // This should never happen due to constructor validation
                        return Err(anyhow::anyhow!(
                            "Non-terminal pipe '{}' produced DirectOutput",
                            pipe.name()
                        ));
                    }
                    return Ok(Some(data));
                }
            }
        }

        // If we reach here, no pipe produced DirectOutput
        // This shouldn't happen with a properly configured pipeline
        Ok(None)
    }

    pub fn subscription_id(&self) -> &str {
        &self.subscription_id
    }
}
