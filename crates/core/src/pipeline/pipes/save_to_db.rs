use super::super::*;
use crate::{cache_input, channel::MessageSender, generated::nostr::fb};
use flatbuffers::FlatBufferBuilder;
use std::sync::Arc;
use tracing::warn;

pub struct SaveToDbPipe {
    to_cache: Arc<dyn MessageSender>,
    name: String,
}

impl SaveToDbPipe {
    pub fn new(to_cache: Arc<dyn MessageSender>) -> Self {
        Self {
            name: "SaveToDb".to_string(),
            to_cache,
        }
    }

    /// Build the WorkerMessage bytes for this event (ParsedEvent if
    /// available, else NostrEvent). Returns the serialized bytes plus a flag
    /// telling whether the content is a ParsedEvent — only then can a
    /// downstream SerializeEventsPipe safely reuse the bytes.
    fn build_message(&self, event: &PipelineEvent) -> Option<(Vec<u8>, bool)> {
        let mut builder = FlatBufferBuilder::new();
        let sub_id_offset = builder.create_string("save_to_db");

        // Determine what to send based on what we have
        let (msg_type, content_type, content_offset, is_parsed) = if let Some(ref parsed) =
            event.parsed
        {
            // Send as ParsedEvent (includes decrypted content for kind4!)
            match parsed.build_flatbuffer(&mut builder) {
                Ok(offset) => (
                    fb::MessageType::ParsedNostrEvent,
                    fb::Message::ParsedEvent,
                    offset.as_union_value(),
                    true,
                ),
                Err(e) => {
                    // Fallback to raw event so we don't drop cache persistence entirely
                    warn!(
                        "Failed to build ParsedEvent flatbuffer (falling back to NostrEvent): {}",
                        e
                    );
                    let offset = parsed.event.build_flatbuffer(&mut builder);
                    (
                        fb::MessageType::NostrEvent,
                        fb::Message::NostrEvent,
                        offset.as_union_value(),
                        false,
                    )
                }
            }
        } else if let Some(ref raw) = event.raw {
            // Send as NostrEvent
            let offset = raw.build_flatbuffer(&mut builder);
            (
                fb::MessageType::NostrEvent,
                fb::Message::NostrEvent,
                offset.as_union_value(),
                false,
            )
        } else {
            // Nothing to send
            return None;
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
        Some((builder.finished_data().to_vec(), is_parsed))
    }
}

impl Pipe for SaveToDbPipe {
    async fn process(&mut self, mut event: PipelineEvent) -> Result<PipeOutput> {
        // Send event as WorkerMessage (ParsedEvent if available, else NostrEvent)
        if let Some((bytes, is_parsed)) = self.build_message(&event) {
            // Frame with the cache-input tagged header so the cache worker can
            // dispatch on the tag and persist these exact bytes.
            let framed = cache_input::frame(cache_input::TAG_PERSIST, &bytes);
            if let Err(e) = self.to_cache.send(&framed) {
                warn!("Failed to send SaveToDb WorkerMessage to cache: {:?}", e);
            }

            // Stash the ParsedEvent WorkerMessage bytes so a downstream
            // SerializeEventsPipe can reuse them instead of serializing again.
            // The inner sub_id ("save_to_db") is ignored downstream: main reads
            // the sub id from the outer tagged framing, and the cache persist
            // path accepts any sub_id since the header-tag framing change.
            if is_parsed {
                event.serialized = Some(bytes);
            }
        }

        // Always pass the event through (with the stash attached when present)
        Ok(PipeOutput::Event(event))
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn run_for_cached_events(&self) -> bool {
        return false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::pipes::serialize_events::SerializeEventsPipe;
    use crate::types::nostr::{Event, EventId, PublicKey};
    use futures::StreamExt;

    fn make_event() -> Event {
        Event {
            id: EventId([1; 32]),
            pubkey: PublicKey([2; 32]),
            created_at: 1_700_000_000,
            kind: 1,
            tags: vec![vec!["t".to_string(), "test".to_string()]],
            content: "hello stash".to_string(),
            sig: hex::encode([4; 64]),
        }
    }

    #[tokio::test]
    async fn stashes_serialized_parsed_event_for_downstream_reuse() {
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<Vec<u8>>();
        let to_cache: Arc<dyn MessageSender> = Arc::new(tx);
        let mut save_pipe = SaveToDbPipe::new(to_cache);

        // Parse through the real parser so ParsedData is present, as the
        // ParsePipe would have done upstream of SaveToDbPipe.
        let parsed = crate::parser::Parser::new(None)
            .parse(make_event())
            .await
            .expect("kind 1 should parse");
        let event = PipelineEvent::from_parsed(parsed);
        let out = save_pipe
            .process(event)
            .await
            .expect("save_to_db should succeed");

        let event = match out {
            PipeOutput::Event(e) => e,
            _ => panic!("SaveToDbPipe must pass the event through"),
        };

        // Stash present and is a ParsedEvent WorkerMessage with sub_id "save_to_db"
        let stashed = event.serialized.clone().expect("parsed events get a stash");
        let wm = flatbuffers::root::<fb::WorkerMessage>(&stashed).expect("valid WorkerMessage");
        assert_eq!(wm.content_type(), fb::Message::ParsedEvent);
        assert_eq!(wm.sub_id(), Some("save_to_db"));

        // Cache received the framed copy of the same bytes
        let framed = rx
            .next()
            .await
            .expect("cache channel should receive the persist frame");
        assert!(framed.len() > stashed.len());
        assert!(framed.ends_with(&stashed[..]));

        // SerializeEventsPipe reuses the stash byte-for-byte
        let mut serialize_pipe = SerializeEventsPipe::new("sub".to_string());
        let out = serialize_pipe
            .process(event)
            .await
            .expect("serialize should succeed");
        match out {
            PipeOutput::DirectOutput(bytes) => assert_eq!(bytes, stashed),
            _ => panic!("SerializeEventsPipe must emit the stashed bytes as DirectOutput"),
        }
    }

    #[tokio::test]
    async fn raw_only_event_gets_no_stash() {
        let (tx, _rx) = futures::channel::mpsc::unbounded::<Vec<u8>>();
        let to_cache: Arc<dyn MessageSender> = Arc::new(tx);
        let mut save_pipe = SaveToDbPipe::new(to_cache);

        let event = PipelineEvent::from_raw(make_event(), None);
        let out = save_pipe
            .process(event)
            .await
            .expect("save_to_db should succeed");

        match out {
            PipeOutput::Event(e) => assert!(e.serialized.is_none()),
            _ => panic!("SaveToDbPipe must pass the event through"),
        }
    }
}
