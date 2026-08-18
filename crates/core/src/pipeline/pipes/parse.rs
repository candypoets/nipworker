use super::super::*;
use crate::types::ParserError;
use hex;
use tracing::warn;

pub struct ParsePipe {
    parser: Arc<Parser>,
    name: String,
}

impl ParsePipe {
    pub fn new(parser: Arc<Parser>) -> Self {
        Self {
            parser,
            name: "Parse".to_string(),
        }
    }
}

impl Pipe for ParsePipe {
    async fn process(&mut self, mut event: PipelineEvent) -> Result<PipeOutput> {
        // If already parsed, just pass through
        if event.is_parsed() {
            return Ok(PipeOutput::Event(event));
        }

        // Parse the raw event
        if let Some(raw_event) = event.raw.take() {
            match self.parser.parse(raw_event).await {
                Ok(parsed_event) => {
                    event.parsed = Some(parsed_event);
                    Ok(PipeOutput::Event(event))
                }
                Err(e @ ParserError::Crypto(_)) => {
                    warn!(
                        "Crypto unavailable while parsing event {}: {}",
                        hex::encode(event.id),
                        e
                    );
                    Err(NostrError::Other(e.to_string()))
                }
                Err(e) => {
                    warn!("Failed to parse event {}: {}", hex::encode(event.id), e);
                    Ok(PipeOutput::Drop)
                }
            }
        } else {
            // No raw event to parse, drop it
            Ok(PipeOutput::Drop)
        }
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn run_for_cached_events(&self) -> bool {
        false
    }
}
