use std::rc::Rc;

use super::super::*;
use flatbuffers::FlatBufferBuilder;
use shared::SabRing;
use tracing::{info, warn};

pub struct SaveToDbPipe {
    db_ring: Rc<RefCell<SabRing>>,
    name: String,
}

impl SaveToDbPipe {
    pub fn new(db_ring: Rc<RefCell<SabRing>>) -> Self {
        Self {
            name: "SaveToDb".to_string(),
            db_ring,
        }
    }
}

impl Pipe for SaveToDbPipe {
    async fn process(&mut self, event: PipelineEvent) -> Result<PipeOutput> {
        let mut fbb = FlatBufferBuilder::new();
        if let Some(ref parsed_event) = event.parsed {
            let fb_parsed_event = match parsed_event.build_flatbuffer(&mut fbb) {
                Ok(parsed_event) => parsed_event,
                Err(e) => {
                    warn!("Failed to build flatbuffer for event: {:?}", e);
                    return Err(NostrError::Parse(e.to_string()));
                }
            };
            fbb.finish(fb_parsed_event, None);
            let bytes = fbb.finished_data();
            let _ = self.db_ring.borrow_mut().write(bytes);
        }
        if let Some(ref nostr_event) = event.raw {
            info!("Saving NostrEvent");
            let fb_raw_event = nostr_event.build_flatbuffer(&mut fbb);
            fbb.finish(fb_raw_event, None);
            let bytes = fbb.finished_data();
            let _ = self.db_ring.borrow_mut().write(bytes);
        }

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
