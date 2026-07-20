//! Adapter between mesh sessions and the dedicated mesh endpoint on the
//! existing CacheWorker.

use flatbuffers::FlatBufferBuilder;
#[cfg(not(target_arch = "wasm32"))]
use nipworker_core::channel::TokioWorkerChannel;
use nipworker_core::cache_input;
use nipworker_core::channel::WorkerChannel;
use nipworker_core::generated::nostr::fb;
use nipworker_core::types::network::Request;

use crate::{CanonicalEvent, MeshError};

const SAVE_TO_DB_SUB_ID: &str = "save_to_db";
const PIN_PROFILE_SUB_ID: &str = "mesh_pin_profile";
const CLEAR_PROFILE_SUB_ID: &str = "mesh_clear_profile";

pub struct MeshCacheClient {
    channel: Box<dyn WorkerChannel>,
    next_request_id: u64,
}

impl MeshCacheClient {
    pub fn new(channel: Box<dyn WorkerChannel>) -> Self {
        Self {
            channel,
            next_request_id: 0,
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_native_channels(
        requests: TokioWorkerChannel,
        responses: TokioWorkerChannel,
    ) -> Self {
        Self::new(Box::new(NativeSplitChannel {
            requests,
            responses,
        }))
    }

    /// Persist a canonical event as WorkerMessage<NostrEvent> in the mesh
    /// storage selected by the CacheWorker's mesh ingress channel.
    pub async fn persist(&self, event: &CanonicalEvent, source: &str) -> Result<(), MeshError> {
        let bytes = build_worker_message(event, SAVE_TO_DB_SUB_ID, source);
        self.channel
            .send(&bytes)
            .await
            .map_err(|_| MeshError::CacheChannelClosed)
    }

    pub async fn pin_profile(&self, event: &CanonicalEvent) -> Result<(), MeshError> {
        let bytes = build_worker_message(event, PIN_PROFILE_SUB_ID, "local://mesh-profile");
        self.channel
            .send(&bytes)
            .await
            .map_err(|_| MeshError::CacheChannelClosed)
    }

    pub async fn clear_profile(&self) -> Result<(), MeshError> {
        let mut builder = FlatBufferBuilder::new();
        let sub_id = builder.create_string(CLEAR_PROFILE_SUB_ID);
        let message = fb::WorkerMessage::create(
            &mut builder,
            &fb::WorkerMessageArgs {
                sub_id: Some(sub_id),
                ..Default::default()
            },
        );
        builder.finish(message, None);
        let framed = cache_input::frame(cache_input::TAG_PERSIST, builder.finished_data());
        self.channel
            .send(&framed)
            .await
            .map_err(|_| MeshError::CacheChannelClosed)
    }

    /// Run the existing cache query engine and return canonical events from
    /// WorkerMessage<NostrEvent> records. An empty CacheResponse terminates
    /// the stored-result stream (mapped to EOSE by the mesh session layer).
    pub async fn query(&mut self, requests: &[Request]) -> Result<Vec<CanonicalEvent>, MeshError> {
        let sub_id = format!("mesh-cache:{}", self.next_request_id);
        self.next_request_id = self.next_request_id.wrapping_add(1);
        let bytes = build_cache_request(&sub_id, requests);
        self.channel
            .send(&bytes)
            .await
            .map_err(|_| MeshError::CacheChannelClosed)?;

        let mut events = Vec::new();
        loop {
            let bytes = self
                .channel
                .recv()
                .await
                .map_err(|_| MeshError::CacheChannelClosed)?;
            let response = flatbuffers::root::<fb::CacheResponse>(&bytes)
                .map_err(|_| MeshError::InvalidCacheResponse)?;
            if response.sub_id() != sub_id {
                return Err(MeshError::UnexpectedCacheResponse {
                    expected: sub_id,
                    actual: response.sub_id().to_string(),
                });
            }
            let payload = response
                .payload()
                .ok_or(MeshError::InvalidCacheResponse)?
                .bytes();
            if payload.is_empty() {
                break;
            }
            decode_batch(payload, &mut events)?;
        }
        Ok(events)
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct NativeSplitChannel {
    requests: TokioWorkerChannel,
    responses: TokioWorkerChannel,
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait::async_trait]
impl WorkerChannel for NativeSplitChannel {
    async fn recv(&mut self) -> Result<Vec<u8>, nipworker_core::channel::ChannelError> {
        self.responses.recv().await
    }

    async fn send(&self, bytes: &[u8]) -> Result<(), nipworker_core::channel::ChannelError> {
        self.requests.send(bytes).await
    }

    fn clone_sender(&self) -> Box<dyn nipworker_core::channel::MessageSender> {
        self.requests.clone_sender()
    }
}

fn build_cache_request(sub_id: &str, requests: &[Request]) -> Vec<u8> {
    let mut builder = FlatBufferBuilder::new();
    let sub_id = builder.create_string(sub_id);
    let request_offsets: Vec<_> = requests
        .iter()
        // The CacheWorker mesh endpoint has no upstream sender, so these
        // requests terminate at mesh storage regardless of client flags.
        .map(|request| request.build_flatbuffer(&mut builder))
        .collect();
    let requests = builder.create_vector(&request_offsets);
    let request = fb::CacheRequest::create(
        &mut builder,
        &fb::CacheRequestArgs {
            sub_id: Some(sub_id),
            requests: Some(requests),
            ..Default::default()
        },
    );
    builder.finish(request, None);
    cache_input::frame(cache_input::TAG_REQUEST, builder.finished_data())
}

fn build_worker_message(event: &CanonicalEvent, sub_id: &str, source: &str) -> Vec<u8> {
    let mut builder = FlatBufferBuilder::new();
    let id = builder.create_string(&event.id);
    let pubkey = builder.create_string(&event.pubkey);
    let content = builder.create_string(&event.content);
    let sig = builder.create_string(&event.sig);
    let tag_offsets: Vec<_> = event
        .tags
        .iter()
        .map(|tag| {
            let items: Vec<_> = tag.iter().map(|item| builder.create_string(item)).collect();
            let items = builder.create_vector(&items);
            fb::StringVec::create(&mut builder, &fb::StringVecArgs { items: Some(items) })
        })
        .collect();
    let tags = builder.create_vector(&tag_offsets);
    let event = fb::NostrEvent::create(
        &mut builder,
        &fb::NostrEventArgs {
            id: Some(id),
            pubkey: Some(pubkey),
            kind: event.kind,
            content: Some(content),
            tags: Some(tags),
            created_at: event.created_at as i32,
            sig: Some(sig),
        },
    );
    let sub_id = builder.create_string(sub_id);
    let source = builder.create_string(source);
    let message = fb::WorkerMessage::create(
        &mut builder,
        &fb::WorkerMessageArgs {
            sub_id: Some(sub_id),
            url: Some(source),
            type_: fb::MessageType::NostrEvent,
            content_type: fb::Message::NostrEvent,
            content: Some(event.as_union_value()),
        },
    );
    builder.finish(message, None);
    cache_input::frame(cache_input::TAG_PERSIST, builder.finished_data())
}

fn decode_batch(payload: &[u8], events: &mut Vec<CanonicalEvent>) -> Result<(), MeshError> {
    let mut offset = 0;
    while offset < payload.len() {
        if payload.len() - offset < 4 {
            return Err(MeshError::InvalidCacheResponse);
        }
        let length = u32::from_le_bytes([
            payload[offset],
            payload[offset + 1],
            payload[offset + 2],
            payload[offset + 3],
        ]) as usize;
        offset += 4;
        let end = offset
            .checked_add(length)
            .filter(|end| *end <= payload.len())
            .ok_or(MeshError::InvalidCacheResponse)?;
        let message = flatbuffers::root::<fb::WorkerMessage>(&payload[offset..end])
            .map_err(|_| MeshError::InvalidCacheResponse)?;
        let event = message
            .content_as_nostr_event()
            .ok_or(MeshError::InvalidCacheRecord)?;
        events.push(canonical_event(event));
        offset = end;
    }
    Ok(())
}

fn canonical_event(event: fb::NostrEvent<'_>) -> CanonicalEvent {
    CanonicalEvent {
        id: event.id().to_string(),
        pubkey: event.pubkey().to_string(),
        created_at: event.created_at().max(0) as u64,
        kind: event.kind(),
        tags: event
            .tags()
            .iter()
            .map(|tag| {
                tag.items()
                    .map(|items| items.iter().map(str::to_string).collect())
                    .unwrap_or_default()
            })
            .collect(),
        content: event.content().to_string(),
        sig: event.sig().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nipworker_core::channel::{TokioWorkerChannel, WorkerChannel};
    use nipworker_core::storage::NostrDbStorage;
    use nipworker_core::worker::cache_worker::CacheWorker;

    fn event(byte: u8) -> CanonicalEvent {
        CanonicalEvent {
            id: hex::encode([byte; 32]),
            pubkey: hex::encode([0xaa; 32]),
            created_at: 100,
            kind: 0,
            tags: vec![],
            content: r#"{"name":"mesh"}"#.to_string(),
            sig: hex::encode([0x55; 64]),
        }
    }

    #[tokio::test]
    async fn persists_and_queries_through_the_mesh_cache_endpoint() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let client_storage = std::sync::Arc::new(NostrDbStorage::new(
                    "test-client".to_string(),
                    1024 * 1024,
                    vec![],
                    vec![],
                ));
                let mesh_storage = std::sync::Arc::new(NostrDbStorage::new(
                    "test-mesh".to_string(),
                    1024 * 1024,
                    vec![],
                    vec![],
                ));
                let worker = CacheWorker::with_mesh_storage(client_storage, mesh_storage);
                let (_parser_tx, parser_rx) = TokioWorkerChannel::new_pair();
                let (parser_results, _parser_results_rx) = TokioWorkerChannel::new_pair();
                let (connections, _connections_rx) = TokioWorkerChannel::new_pair();
                let (mesh_client, mesh_worker) = TokioWorkerChannel::new_pair();
                let (mesh_results, mesh_results_rx) = TokioWorkerChannel::new_pair();

                worker.run_with_mesh(
                    Box::new(parser_rx),
                    parser_results.clone_sender(),
                    connections.clone_sender(),
                    Box::new(mesh_worker),
                    mesh_results.clone_sender(),
                );

                // The endpoint uses separate request and response channels;
                // combine their sending/receiving halves for the client.
                let mut client = MeshCacheClient::new(Box::new(SplitChannel {
                    requests: mesh_client,
                    responses: mesh_results_rx,
                }));
                let expected = event(0x11);
                client.persist(&expected, "ble://peer-a").await.unwrap();
                tokio::task::yield_now().await;

                let request = Request {
                    authors: vec![expected.pubkey.clone()],
                    kinds: vec![0],
                    cache_only: true,
                    ..Default::default()
                };
                let events = client.query(&[request]).await.unwrap();
                assert_eq!(events, vec![expected]);
            })
            .await;
    }

    struct SplitChannel {
        requests: TokioWorkerChannel,
        responses: TokioWorkerChannel,
    }

    #[async_trait::async_trait]
    impl WorkerChannel for SplitChannel {
        async fn recv(&mut self) -> Result<Vec<u8>, nipworker_core::channel::ChannelError> {
            self.responses.recv().await
        }

        async fn send(&self, bytes: &[u8]) -> Result<(), nipworker_core::channel::ChannelError> {
            self.requests.send(bytes).await
        }

        fn clone_sender(&self) -> Box<dyn nipworker_core::channel::MessageSender> {
            self.requests.clone_sender()
        }
    }
}
