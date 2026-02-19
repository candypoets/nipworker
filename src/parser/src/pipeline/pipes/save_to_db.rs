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

    /// Send a NostrEvent flatbuffer to the cache worker as a CacheRequest
    fn send_nostr_event_to_cache(&self, event_offset: flatbuffers::WIPOffset<fb::NostrEvent<'_>>) {
        let mut builder = FlatBufferBuilder::new();

        // Create the subscription id string
        let sub_id_offset = builder.create_string("save_to_db");

        // Create the CacheRequest with only the event field set
        let cache_req = fb::CacheRequest::create(
            &mut builder,
            &fb::CacheRequestArgs {
                sub_id: Some(sub_id_offset),
                requests: None,
                event: Some(event_offset),
                relays: None,
            },
        );

        builder.finish(cache_req, None);
        let bytes = builder.finished_data().to_vec();

        // Send through the MessageChannel port
        let _ = self.to_cache.borrow().send(&bytes);
    }
}

impl Pipe for SaveToDbPipe {
    async fn process(&mut self, event: PipelineEvent) -> Result<PipeOutput> {
        // Send the event to cache - prefer raw event, fall back to parsed_event.event
        if let Some(ref nostr_event) = event.raw {
            info!("Saving NostrEvent from raw");
            let mut fbb = FlatBufferBuilder::new();
            let fb_raw_event = nostr_event.build_flatbuffer(&mut fbb);
            self.send_nostr_event_to_cache(fb_raw_event);
        } else if let Some(ref parsed_event) = event.parsed {
            info!("Saving NostrEvent from parsed");
            let mut fbb = FlatBufferBuilder::new();
            let fb_event = parsed_event.event.build_flatbuffer(&mut fbb);
            self.send_nostr_event_to_cache(fb_event);
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
