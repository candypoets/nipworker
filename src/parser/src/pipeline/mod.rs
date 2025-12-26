use crate::parser::Parser;
use crate::types::parsed_event::ParsedEvent;
use crate::utils::json::extract_event_id;
use crate::NostrError;
use crypto::CryptoClient;
use shared::generated::nostr::fb::{self};
use shared::types::Event;

type Result<T> = std::result::Result<T, NostrError>;

use hex::decode_to_slice;
use rustc_hash::FxHashSet;
use shared::SabRing;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use tracing::info;
use wasm_bindgen::prelude::*;

pub mod pipes;

pub use pipes::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = JSON)]
    fn parse(s: &str) -> JsValue;
}

pub struct PipelineEvent {
    /// Raw nostr event (if available)
    pub raw: Option<Event>,
    /// Parsed event (if already parsed)
    pub parsed: Option<ParsedEvent>,
    /// Event ID (always available for deduplication)
    pub id: [u8; 32],
    /// Relay source (if from network)
    pub source_relay: Option<String>,
}

impl PipelineEvent {
    pub fn from_raw(event: Event, source_relay: Option<String>) -> Self {
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

pub enum PipeOutput {
    /// Continue with this event
    Event(PipelineEvent),
    /// Drop the event
    Drop,
    /// Send custom data directly to buffer
    DirectOutput(Vec<u8>),
}

/// A pipe in the processing pipeline
pub trait Pipe {
    async fn process(&mut self, event: PipelineEvent) -> Result<PipeOutput>;
    async fn process_cached_batch(&mut self, messages: &[Vec<u8>]) -> Result<Vec<Vec<u8>>> {
        Ok(messages.to_vec())
    }
    fn name(&self) -> &str;

    /// Whether this pipe can produce DirectOutput (only last pipe should)
    fn can_direct_output(&self) -> bool {
        false
    }

    fn run_for_cached_events(&self) -> bool {
        true
    }
}

/// Enum representing all possible pipe types to avoid dynamic dispatch
pub enum PipeType {
    Parse(ParsePipe),
    SaveToDb(SaveToDbPipe),
    SerializeEvents(SerializeEventsPipe),
    ProofVerification(ProofVerificationPipe),
    Counter(CounterPipe),
    KindFilter(KindFilterPipe),
    NpubLimiter(NpubLimiterPipe),
    MuteFilter(MuteFilterPipe),
}

impl PipeType {
    pub async fn process(&mut self, event: PipelineEvent) -> Result<PipeOutput> {
        match self {
            PipeType::Parse(pipe) => pipe.process(event).await,
            PipeType::SaveToDb(pipe) => pipe.process(event).await,
            PipeType::SerializeEvents(pipe) => pipe.process(event).await,
            PipeType::ProofVerification(pipe) => pipe.process(event).await,
            PipeType::Counter(pipe) => pipe.process(event).await,
            PipeType::KindFilter(pipe) => pipe.process(event).await,
            PipeType::NpubLimiter(pipe) => pipe.process(event).await,
            PipeType::MuteFilter(pipe) => pipe.process(event).await,
        }
    }

    pub async fn process_cached_batch(&mut self, messages: &[Vec<u8>]) -> Result<Vec<Vec<u8>>> {
        match self {
            PipeType::Parse(pipe) => {
                <ParsePipe as Pipe>::process_cached_batch(pipe, messages).await
            }
            PipeType::SaveToDb(pipe) => {
                <SaveToDbPipe as Pipe>::process_cached_batch(pipe, messages).await
            }
            PipeType::SerializeEvents(pipe) => {
                <SerializeEventsPipe as Pipe>::process_cached_batch(pipe, messages).await
            }
            PipeType::ProofVerification(pipe) => {
                <ProofVerificationPipe as Pipe>::process_cached_batch(pipe, messages).await
            }
            PipeType::Counter(pipe) => {
                <CounterPipe as Pipe>::process_cached_batch(pipe, messages).await
            }
            PipeType::KindFilter(pipe) => {
                <KindFilterPipe as Pipe>::process_cached_batch(pipe, messages).await
            }
            PipeType::NpubLimiter(pipe) => {
                <NpubLimiterPipe as Pipe>::process_cached_batch(pipe, messages).await
            }
            PipeType::MuteFilter(pipe) => {
                <MuteFilterPipe as Pipe>::process_cached_batch(pipe, messages).await
            }
        }
    }

    pub fn name(&self) -> &str {
        match self {
            PipeType::Parse(pipe) => pipe.name(),
            PipeType::SaveToDb(pipe) => pipe.name(),
            PipeType::SerializeEvents(pipe) => pipe.name(),
            PipeType::ProofVerification(pipe) => pipe.name(),
            PipeType::Counter(pipe) => pipe.name(),
            PipeType::KindFilter(pipe) => pipe.name(),
            PipeType::NpubLimiter(pipe) => pipe.name(),
            PipeType::MuteFilter(pipe) => pipe.name(),
        }
    }

    pub fn can_direct_output(&self) -> bool {
        match self {
            PipeType::Parse(pipe) => pipe.can_direct_output(),
            PipeType::SaveToDb(pipe) => pipe.can_direct_output(),
            PipeType::SerializeEvents(pipe) => pipe.can_direct_output(),
            PipeType::ProofVerification(pipe) => pipe.can_direct_output(),
            PipeType::Counter(pipe) => pipe.can_direct_output(),
            PipeType::KindFilter(pipe) => pipe.can_direct_output(),
            PipeType::NpubLimiter(pipe) => pipe.can_direct_output(),
            PipeType::MuteFilter(pipe) => pipe.can_direct_output(),
        }
    }

    pub fn run_for_cached_events(&self) -> bool {
        match self {
            PipeType::Parse(pipe) => pipe.run_for_cached_events(),
            PipeType::SaveToDb(pipe) => pipe.run_for_cached_events(),
            PipeType::SerializeEvents(pipe) => pipe.run_for_cached_events(),
            PipeType::ProofVerification(pipe) => pipe.run_for_cached_events(),
            PipeType::Counter(pipe) => pipe.run_for_cached_events(),
            PipeType::KindFilter(pipe) => pipe.run_for_cached_events(),
            PipeType::NpubLimiter(pipe) => pipe.run_for_cached_events(),
            PipeType::MuteFilter(pipe) => pipe.run_for_cached_events(),
        }
    }
}

/// Pipeline processor
pub struct Pipeline {
    pipes: Vec<PipeType>,
    subscription_id: String,
    seen_ids: RefCell<FxHashSet<[u8; 32]>>,
    dedup_max_size: usize,
}

impl Pipeline {
    pub fn new(pipes: Vec<PipeType>, subscription_id: String) -> Result<Self> {
        // Validate pipeline: only last pipe can direct output
        for (i, pipe) in pipes.iter().enumerate() {
            let is_last = i == pipes.len() - 1;
            if pipe.can_direct_output() && !is_last {
                return Err(NostrError::Other(format!(
                    "Pipe '{}' can produce DirectOutput but is not the last pipe in pipeline",
                    pipe.name()
                )));
            }
        }

        Ok(Self {
            pipes,
            subscription_id,
            seen_ids: RefCell::new(FxHashSet::with_capacity_and_hasher(
                10_000,
                Default::default(),
            )),
            dedup_max_size: 10_000,
        })
    }

    /// Create default pipeline: parsing + save to db + serialize events
    pub fn default(
        parser: Arc<Parser>,
        db_ring: Rc<RefCell<SabRing>>,
        subscription_id: String,
    ) -> Result<Self> {
        Self::new(
            vec![
                PipeType::MuteFilter(MuteFilterPipe::new(MuteCriteria::new(
                    vec![],
                    vec!["nsfw".to_string()],
                    vec!["nsfw".to_string()],
                    vec![],
                ))),
                PipeType::Parse(ParsePipe::new(parser)),
                PipeType::SaveToDb(SaveToDbPipe::new(db_ring)),
                PipeType::SerializeEvents(SerializeEventsPipe::new(subscription_id.clone())),
            ],
            subscription_id,
        )
    }

    /// Create proof verification pipeline: parsing + proof verification
    pub fn proof_verification(
        parser: Arc<Parser>,
        crypto_client: Arc<CryptoClient>,
        subscription_id: String,
        max_proofs: usize,
    ) -> Result<Self> {
        Self::new(
            vec![
                PipeType::KindFilter(KindFilterPipe::new(vec![9321, 7375])), // Only process cashu events
                PipeType::Parse(ParsePipe::new(parser)),
                PipeType::ProofVerification(ProofVerificationPipe::new(max_proofs, crypto_client)),
            ],
            subscription_id,
        )
    }

    /// Process a single event through the pipeline
    pub async fn process(&mut self, raw_event_json: &str) -> Result<Option<Vec<u8>>> {
        // 1️⃣ Extract id
        let id_hex = match extract_event_id(raw_event_json) {
            Some(id) => id,
            None => {
                tracing::warn!("No id field found in incoming event");
                return Ok(None);
            }
        };

        // 2️⃣ Decode hex to [u8; 32]
        let mut id_bytes = [0u8; 32];
        if decode_to_slice(id_hex, &mut id_bytes).is_err() {
            tracing::warn!("Invalid hex in event id: {}", id_hex);
            return Ok(None);
        }

        // 3️⃣ Deduplicate
        {
            let mut seen = self.seen_ids.borrow_mut();
            if seen.contains(&id_bytes) {
                return Ok(None); // already processed
            }
            if seen.len() < self.dedup_max_size {
                seen.insert(id_bytes);
            }
        }

        // 4️⃣ Parse full crate::types::nostr::Event
        let nostr_event = match Event::from_json(raw_event_json) {
            Ok(ev) => ev,
            Err(e) => {
                return Ok(None);
            }
        };

        let mut event = PipelineEvent::from_raw(nostr_event, None);

        // 5️⃣ Run through pipes
        let pipes_len = self.pipes.len();

        for (i, pipe) in self.pipes.iter_mut().enumerate() {
            let is_last = i == pipes_len - 1;
            match pipe.process(event).await? {
                PipeOutput::Event(e) => event = e,
                PipeOutput::Drop => return Ok(None),
                PipeOutput::DirectOutput(data) => {
                    if !is_last {
                        return Err(NostrError::Other(format!(
                            "Non-terminal pipe '{}' produced DirectOutput",
                            pipe.name()
                        )));
                    }
                    return Ok(Some(data));
                }
            }
        }

        Ok(None)
    }

    /// Process a single event through the pipeline from FlatBuffer bytes
    pub async fn process_bytes(&mut self, raw_event_bytes: &[u8]) -> Result<Option<Vec<u8>>> {
        info!("Processing event bytes");

        // Parse the flatbuffer root
        let fb_worker_msg = match shared::generated::message::nostr::fb::root_as_worker_message(raw_event_bytes) {
            Ok(msg) => msg,
            Err(_) => {
                tracing::warn!("Failed to parse worker message from flatbuffer bytes");
                return Ok(None);
            }
        };

        // Extract the event from the worker message
        let fb_event = match fb_worker_msg.content_as_nostr_event() {
            Some(event) => event,
            None => {
                tracing::warn!("Worker message does not contain a NostrEvent");
                return Ok(None);
            }
        };

        let nostr_event = match Event::from_flatbuffer(&fb_event) {
            Ok(ev) => ev,
            Err(_) => {
                tracing::warn!("Failed to convert flatbuffer to Event");
                return Ok(None);
            }
        };

        // 2️⃣ Use EventId bytes directly
        let id_bytes = nostr_event.id.to_bytes();

        // 3️⃣ Deduplicate
        {
            let mut seen = self.seen_ids.borrow_mut();
            if seen.contains(&id_bytes) {
                return Ok(None); // already processed
            }
            if seen.len() < self.dedup_max_size {
                seen.insert(id_bytes);
            }
        }

        let mut event = PipelineEvent::from_raw(nostr_event, None);

        // 5️⃣ Run through pipes
        let pipes_len = self.pipes.len();

        for (i, pipe) in self.pipes.iter_mut().enumerate() {
            let is_last = i == pipes_len - 1;
            match pipe.process(event).await? {
                PipeOutput::Event(e) => event = e,
                PipeOutput::Drop => return Ok(None),
                PipeOutput::DirectOutput(data) => {
                    if !is_last {
                        return Err(NostrError::Other(format!(
                            "Non-terminal pipe '{}' produced DirectOutput",
                            pipe.name()
                        )));
                    }
                    return Ok(Some(data));
                }
            }
        }

        Ok(None)
    }

    /// Process a batch of cached WorkerMessage bytes through cache-capable pipes.
    pub async fn process_cached_batch(&mut self, messages: &[Vec<u8>]) -> Result<Vec<Vec<u8>>> {
        for message in messages {
            self.mark_as_seen(message);
        }
        let mut outputs = Vec::new();
        for pipe in self.pipes.iter_mut() {
            if pipe.run_for_cached_events() {
                let mut out = pipe.process_cached_batch(messages).await?;
                outputs.append(&mut out);
            }
        }
        Ok(outputs)
    }

    fn extract_parsed_event(worker_message: &Vec<u8>) -> Option<fb::ParsedEvent<'_>> {
        if let Ok(message) = flatbuffers::root::<fb::WorkerMessage>(&worker_message) {
            match message.content_type() {
                fb::Message::ParsedEvent => message.content_as_parsed_event(),
                _ => None,
            }
        } else {
            None
        }
    }

    pub fn mark_as_seen(&self, message_bytes: &Vec<u8>) {
        if let Some(event) = Self::extract_parsed_event(message_bytes) {
            let id_hex = event.id();

            let mut id_bytes = [0u8; 32];
            if decode_to_slice(id_hex, &mut id_bytes).is_err() {
                tracing::warn!("Invalid hex in event id (mark_as_seen): {}", id_hex);
                return;
            }

            let mut seen = self.seen_ids.borrow_mut();
            if seen.contains(&id_bytes) {
                return; // already processed
            }
            if seen.len() < self.dedup_max_size {
                seen.insert(id_bytes);
            }
        }
    }

    pub fn subscription_id(&self) -> &str {
        &self.subscription_id
    }
}
