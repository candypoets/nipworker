use super::super::*;
use crate::network::interfaces::EventDatabase;
use nostr::util::hex;

pub struct SaveToDbPipe {
    database: Arc<NostrDB>,
    name: String,
}

impl SaveToDbPipe {
    pub fn new(database: Arc<NostrDB>) -> Self {
        Self {
            database,
            name: "SaveToDb".to_string(),
        }
    }
}

impl Pipe for SaveToDbPipe {
    async fn process(&mut self, event: PipelineEvent) -> Result<PipeOutput> {
        if let Some(ref parsed_event) = event.parsed {
            let _ = self.database.add_event(parsed_event.clone()).await;
            tracing::debug!("Saved event {} to database", hex::encode(event.id));
        }

        // Always pass the event through unchanged
        Ok(PipeOutput::Event(event))
    }

    fn name(&self) -> &str {
        &self.name
    }
}
