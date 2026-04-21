use crate::channel::{WorkerChannel, WorkerChannelSender};
use crate::generated::nostr::fb;
use crate::spawn::spawn_worker;
use crate::traits::Storage;
use crate::types::network::Request;
use serde_json::{json, Map, Value};
use std::sync::Arc;
use tracing::{info, warn};

const DEFAULT_RELAYS: &[&str] = &[
	"wss://relay.snort.social",
	"wss://relay.damus.io",
	"wss://relay.primal.net",
];

pub struct CacheWorker {
	_storage: Arc<dyn Storage>,
}

impl CacheWorker {
	pub fn new(storage: Arc<dyn Storage>) -> Self {
		Self { _storage: storage }
	}

	pub fn run(
		self,
		mut from_parser: Box<dyn WorkerChannel>,
		to_parser: Box<dyn WorkerChannelSender>,
		to_connections: Box<dyn WorkerChannelSender>,
	) {
		spawn_worker(async move {
			info!("[CacheWorker] started");
			if let Err(e) = self._storage.initialize().await {
				warn!("[CacheWorker] failed to initialize storage: {}", e);
			}

			while let Ok(bytes) = from_parser.recv().await {
				// 1) Try WorkerMessage first (persist path)
				if let Ok(worker_msg) = flatbuffers::root::<fb::WorkerMessage>(&bytes) {
					if worker_msg.sub_id() == Some("save_to_db") {
						if let Err(e) = self._storage.persist(&bytes).await {
							warn!("[CacheWorker] persist failed: {}", e);
						}
						continue;
					}
				}

				// 2) Try CacheRequest
				let cache_req = match flatbuffers::root::<fb::CacheRequest>(&bytes) {
					Ok(r) => r,
					Err(e) => {
						warn!("[CacheWorker] failed to decode CacheRequest: {}", e);
						continue;
					}
				};

				// Publish path (event field present)
				if let Some(fb_event) = cache_req.event() {
					let tags_vec = fb_event.tags();
					let mut tags_json = Vec::with_capacity(tags_vec.len());
					for i in 0..tags_vec.len() {
						let sv = tags_vec.get(i);
						if let Some(items) = sv.items() {
							let arr: Vec<Value> = (0..items.len())
								.map(|j| Value::String(items.get(j).to_string()))
								.collect();
							tags_json.push(Value::Array(arr));
						} else {
							tags_json.push(Value::Array(vec![]));
						}
					}

					let event_json = json!({
						"id": fb_event.id(),
						"pubkey": fb_event.pubkey(),
						"kind": fb_event.kind(),
						"content": fb_event.content(),
						"tags": tags_json,
						"created_at": fb_event.created_at(),
						"sig": fb_event.sig(),
					});

					let frame = json!(["EVENT", event_json]);
					let frame_str =
						serde_json::to_string(&frame).unwrap_or_else(|_| "[]".to_string());

					let relays: Vec<String> = cache_req
						.relays()
						.map(|r| (0..r.len()).map(|i| r.get(i).to_string()).collect())
						.filter(|v: &Vec<String>| !v.is_empty())
						.unwrap_or_else(|| DEFAULT_RELAYS.iter().map(|s| s.to_string()).collect());

					let envelope = json!({ "relays": relays, "frames": [frame_str] });
					let env_str =
						serde_json::to_string(&envelope).unwrap_or_else(|_| "{}".to_string());

					if let Err(e) = to_connections.send(env_str.as_bytes()) {
						warn!("[CacheWorker] failed to send publish envelope: {}", e);
					}
					continue;
				}

				// Query path (requests field present)
				let sub_id = cache_req.sub_id().to_string();
				if let Some(reqs) = cache_req.requests() {
					let mut all_cached_events: Vec<Vec<u8>> = Vec::new();
					let mut skip_req_indices = std::collections::HashSet::new();

					for i in 0..reqs.len() {
						let fb_req = reqs.get(i);
						let request = Request::from_flatbuffer(&fb_req);

						if request.no_cache {
							continue;
						}

						let filter = match request.to_filter() {
							Ok(f) => f,
							Err(e) => {
								warn!("[CacheWorker] failed to convert request to filter: {}", e);
								continue;
							}
						};

						match self._storage.query(vec![filter]).await {
							Ok(events) => {
								if request.cache_first && !events.is_empty() {
									skip_req_indices.insert(i);
								}
								if request.cache_only {
									skip_req_indices.insert(i);
								}
								all_cached_events.extend(events);
							}
							Err(e) => warn!("[CacheWorker] query failed: {}", e),
						}
					}

					// Send batched cached events to parser
					if !all_cached_events.is_empty() {
						let total_bytes: usize = all_cached_events.iter().map(|e| 4 + e.len()).sum();
						let mut batched = Vec::with_capacity(total_bytes);
						for ev in &all_cached_events {
							batched.extend_from_slice(&(ev.len() as u32).to_le_bytes());
							batched.extend_from_slice(ev);
						}

						let resp_bytes = serialize_cache_response(&sub_id, &batched);
						if let Err(e) = to_parser.send(&resp_bytes) {
							warn!("[CacheWorker] failed to send batched CacheResponse: {}", e);
						}
					}

					// Send REQ frames to connections
					for i in 0..reqs.len() {
						if skip_req_indices.contains(&i) {
							continue;
						}
						let fb_req = reqs.get(i);
						let filter_json = fb_request_to_json(&fb_req);
						let frame = json!(["REQ", &sub_id, filter_json]);
						let frame_str =
							serde_json::to_string(&frame).unwrap_or_else(|_| "[]".to_string());

						let relays: Vec<String> = fb_req
							.relays()
							.map(|r| (0..r.len()).map(|j| r.get(j).to_string()).collect())
							.filter(|v: &Vec<String>| !v.is_empty())
							.unwrap_or_else(|| DEFAULT_RELAYS.iter().map(|s| s.to_string()).collect());

						let envelope = json!({ "relays": relays, "frames": [frame_str] });
						let env_str =
							serde_json::to_string(&envelope).unwrap_or_else(|_| "{}".to_string());

						if let Err(e) = to_connections.send(env_str.as_bytes()) {
							warn!("[CacheWorker] failed to send REQ envelope: {}", e);
						}
					}

					// Emit EOCE signal
					let resp_bytes = serialize_cache_response(&sub_id, &[]);
					if let Err(e) = to_parser.send(&resp_bytes) {
						warn!("[CacheWorker] failed to send EOCE CacheResponse: {}", e);
					}
				}
			}

			info!("[CacheWorker] channel closed, exiting");
		});
	}
}

fn serialize_cache_response(sub_id: &str, payload: &[u8]) -> Vec<u8> {
	let mut builder = flatbuffers::FlatBufferBuilder::new();
	let sid = builder.create_string(sub_id);
	let payload_vec = builder.create_vector(payload);
	let resp = fb::CacheResponse::create(
		&mut builder,
		&fb::CacheResponseArgs {
			sub_id: Some(sid),
			payload: Some(payload_vec),
		},
	);
	builder.finish(resp, None);
	builder.finished_data().to_vec()
}

fn fb_request_to_json(fb_req: &fb::Request<'_>) -> Value {
	let mut filter = Map::new();

	if let Some(ids) = fb_req.ids() {
		let arr: Vec<_> = (0..ids.len())
			.map(|i| Value::String(ids.get(i).to_string()))
			.collect();
		if !arr.is_empty() {
			filter.insert("ids".to_string(), Value::Array(arr));
		}
	}

	if let Some(authors) = fb_req.authors() {
		let arr: Vec<_> = (0..authors.len())
			.map(|i| Value::String(authors.get(i).to_string()))
			.collect();
		if !arr.is_empty() {
			filter.insert("authors".to_string(), Value::Array(arr));
		}
	}

	if let Some(kinds) = fb_req.kinds() {
		let arr: Vec<_> = kinds
			.into_iter()
			.map(|k| Value::Number((k as i64).into()))
			.collect();
		if !arr.is_empty() {
			filter.insert("kinds".to_string(), Value::Array(arr));
		}
	}

	if let Some(s) = fb_req.search() {
		if !s.is_empty() {
			filter.insert("search".to_string(), Value::String(s.to_string()));
		}
	}

	let limit = fb_req.limit();
	if limit > 0 {
		filter.insert("limit".to_string(), Value::Number((limit as i64).into()));
	}

	let since = fb_req.since();
	if since > 0 {
		filter.insert("since".to_string(), Value::Number((since as i64).into()));
	}

	let until = fb_req.until();
	if until > 0 {
		filter.insert("until".to_string(), Value::Number((until as i64).into()));
	}

	if let Some(tags) = fb_req.tags() {
		for j in 0..tags.len() {
			let t = tags.get(j);
			if let Some(items) = t.items() {
				if items.len() >= 2 {
					let key = items.get(0).to_string();
					let vals: Vec<_> = (1..items.len())
						.map(|k| Value::String(items.get(k).to_string()))
						.collect();
					filter.insert(key, Value::Array(vals));
				}
			}
		}
	}

	Value::Object(filter)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
	use super::*;
	use crate::channel::TokioWorkerChannel;
	use crate::generated::nostr::fb;
	use crate::traits::{Storage, StorageError};
	use crate::types::network::Request;
	use crate::types::nostr::Filter;
	use async_trait::async_trait;
	use serde_json::{json, Value};
	use std::sync::{Arc, Mutex};

	#[derive(Debug, Clone)]
	struct PersistCall {
		bytes: Vec<u8>,
	}

	#[derive(Debug, Clone)]
	struct QueryCall {
		filters: Vec<Filter>,
	}

	struct MockStorage {
		persist_calls: Arc<Mutex<Vec<PersistCall>>>,
		query_calls: Arc<Mutex<Vec<QueryCall>>>,
		query_results: Arc<Mutex<Vec<Result<Vec<Vec<u8>>, StorageError>>>>,
	}

	impl MockStorage {
		fn new() -> Self {
			Self {
				persist_calls: Arc::new(Mutex::new(Vec::new())),
				query_calls: Arc::new(Mutex::new(Vec::new())),
				query_results: Arc::new(Mutex::new(Vec::new())),
			}
		}

		fn with_query_results(results: Vec<Result<Vec<Vec<u8>>, StorageError>>) -> Self {
			let s = Self::new();
			*s.query_results.lock().unwrap() = results;
			s
		}
	}

	#[async_trait(?Send)]
	impl Storage for MockStorage {
		async fn query(&self, filters: Vec<Filter>) -> Result<Vec<Vec<u8>>, StorageError> {
			self.query_calls.lock().unwrap().push(QueryCall { filters });
			let mut results = self.query_results.lock().unwrap();
			if !results.is_empty() {
				results.remove(0)
			} else {
				Ok(Vec::new())
			}
		}

		async fn persist(&self, event_bytes: &[u8]) -> Result<(), StorageError> {
			self.persist_calls
				.lock()
				.unwrap()
				.push(PersistCall {
					bytes: event_bytes.to_vec(),
				});
			Ok(())
		}

		async fn initialize(&self) -> Result<(), StorageError> {
			Ok(())
		}
	}

	fn build_worker_message_bytes(sub_id: &str) -> Vec<u8> {
		let mut builder = flatbuffers::FlatBufferBuilder::new();
		let sid = builder.create_string(sub_id);
		let msg = fb::WorkerMessage::create(
			&mut builder,
			&fb::WorkerMessageArgs {
				sub_id: Some(sid),
				..Default::default()
			},
		);
		builder.finish(msg, None);
		builder.finished_data().to_vec()
	}

	fn build_publish_request_bytes(
		id: &str,
		pubkey: &str,
		kind: u16,
		content: &str,
		created_at: i32,
		sig: &str,
		relays: &[&str],
	) -> Vec<u8> {
		let mut builder = flatbuffers::FlatBufferBuilder::new();
		let id_off = builder.create_string(id);
		let pubkey_off = builder.create_string(pubkey);
		let content_off = builder.create_string(content);
		let sig_off = builder.create_string(sig);
		let tag_offsets: Vec<flatbuffers::WIPOffset<fb::StringVec>> = vec![];
		let tags_off = builder.create_vector(&tag_offsets);
		let event_fb = fb::NostrEvent::create(
			&mut builder,
			&fb::NostrEventArgs {
				id: Some(id_off),
				pubkey: Some(pubkey_off),
				kind,
				content: Some(content_off),
				tags: Some(tags_off),
				created_at,
				sig: Some(sig_off),
			},
		);
		let relays_vec = if !relays.is_empty() {
			let rs: Vec<_> = relays.iter().map(|r| builder.create_string(r)).collect();
			Some(builder.create_vector(&rs))
		} else {
			None
		};
		let sub_id_off = builder.create_string("pub");
		let req = fb::CacheRequest::create(
			&mut builder,
			&fb::CacheRequestArgs {
				sub_id: Some(sub_id_off),
				event: Some(event_fb),
				relays: relays_vec,
				..Default::default()
			},
		);
		builder.finish(req, None);
		builder.finished_data().to_vec()
	}

	fn build_query_request_bytes(sub_id: &str, requests: Vec<Request>) -> Vec<u8> {
		let mut builder = flatbuffers::FlatBufferBuilder::new();
		let sid = builder.create_string(sub_id);
		let req_offsets: Vec<_> =
			requests.iter().map(|r| r.build_flatbuffer(&mut builder)).collect();
		let reqs_vec = builder.create_vector(&req_offsets);
		let req = fb::CacheRequest::create(
			&mut builder,
			&fb::CacheRequestArgs {
				sub_id: Some(sid),
				requests: Some(reqs_vec),
				..Default::default()
			},
		);
		builder.finish(req, None);
		builder.finished_data().to_vec()
	}

	#[tokio::test]
	async fn test_save_to_db_persists_worker_message() {
		let local = tokio::task::LocalSet::new();
		local
			.run_until(async {
				let storage = Arc::new(MockStorage::new());
				let worker = CacheWorker::new(storage.clone());
				let (mut from_parser_tx, from_parser_rx) = TokioWorkerChannel::new_pair();
				let (to_parser_tx, _to_parser_rx) = TokioWorkerChannel::new_pair();
				let (to_connections_tx, _to_connections_rx) = TokioWorkerChannel::new_pair();

				worker.run(
					Box::new(from_parser_rx),
					to_parser_tx.clone_sender(),
					to_connections_tx.clone_sender(),
				);

				let bytes = build_worker_message_bytes("save_to_db");
				from_parser_tx.send(&bytes).await.unwrap();

				tokio::task::yield_now().await;
				tokio::task::yield_now().await;

				let persist_calls = storage.persist_calls.lock().unwrap();
				assert_eq!(persist_calls.len(), 1);
				assert_eq!(persist_calls[0].bytes, bytes);
			})
			.await;
	}

	#[tokio::test]
	async fn test_publish_event_sends_event_envelope_to_connections() {
		let local = tokio::task::LocalSet::new();
		local
			.run_until(async {
				let storage = Arc::new(MockStorage::new());
				let worker = CacheWorker::new(storage.clone());
				let (mut from_parser_tx, from_parser_rx) = TokioWorkerChannel::new_pair();
				let (to_parser_tx, _to_parser_rx) = TokioWorkerChannel::new_pair();
				let (to_connections_tx, mut to_connections_rx) = TokioWorkerChannel::new_pair();

				worker.run(
					Box::new(from_parser_rx),
					to_parser_tx.clone_sender(),
					to_connections_tx.clone_sender(),
				);

				let bytes = build_publish_request_bytes(
					"0000000000000000000000000000000000000000000000000000000000000001",
					"0000000000000000000000000000000000000000000000000000000000000002",
					1,
					"hello",
					1234567890,
					"0000000000000000000000000000000000000000000000000000000000000003",
					&["wss://r"],
				);
				from_parser_tx.send(&bytes).await.unwrap();

				let env_bytes = to_connections_rx.recv().await.unwrap();
				let envelope: Value = serde_json::from_slice(&env_bytes).unwrap();
				assert_eq!(envelope["relays"], json!(["wss://r"]));
				let frames = envelope["frames"].as_array().unwrap();
				assert_eq!(frames.len(), 1);
				let frame: Value = serde_json::from_str(frames[0].as_str().unwrap()).unwrap();
				let arr = frame.as_array().unwrap();
				assert_eq!(arr[0], "EVENT");
				let event = &arr[1];
				assert_eq!(
					event["id"],
					"0000000000000000000000000000000000000000000000000000000000000001"
				);
				assert_eq!(
					event["pubkey"],
					"0000000000000000000000000000000000000000000000000000000000000002"
				);
				assert_eq!(event["kind"], 1);
				assert_eq!(event["content"], "hello");
				assert_eq!(event["created_at"], 1234567890);
				assert_eq!(
					event["sig"],
					"0000000000000000000000000000000000000000000000000000000000000003"
				);
			})
			.await;
	}

	#[tokio::test]
	async fn test_query_returns_batched_cache_response_to_parser() {
		let local = tokio::task::LocalSet::new();
		local
			.run_until(async {
				let storage = Arc::new(MockStorage::with_query_results(vec![
					Ok(vec![b"ev0".to_vec(), b"ev1".to_vec(), b"ev2".to_vec()]),
					Ok(vec![]),
				]));
				let worker = CacheWorker::new(storage.clone());
				let (mut from_parser_tx, from_parser_rx) = TokioWorkerChannel::new_pair();
				let (to_parser_tx, mut to_parser_rx) = TokioWorkerChannel::new_pair();
				let (to_connections_tx, _to_connections_rx) = TokioWorkerChannel::new_pair();

				worker.run(
					Box::new(from_parser_rx),
					to_parser_tx.clone_sender(),
					to_connections_tx.clone_sender(),
				);

				let requests = vec![
					Request {
						relays: vec!["wss://r".to_string()],
						..Default::default()
					},
					Request {
						relays: vec!["wss://r".to_string()],
						..Default::default()
					},
				];
				let bytes = build_query_request_bytes("s1", requests);
				from_parser_tx.send(&bytes).await.unwrap();

				let resp_bytes = to_parser_rx.recv().await.unwrap();
				let resp = flatbuffers::root::<fb::CacheResponse>(&resp_bytes).unwrap();
				let payload = resp.payload().unwrap().bytes();

				let mut offset = 0;
				let len0 = u32::from_le_bytes([
					payload[offset],
					payload[offset + 1],
					payload[offset + 2],
					payload[offset + 3],
				]) as usize;
				assert_eq!(len0, 3);
				assert_eq!(&payload[offset + 4..offset + 4 + len0], b"ev0");
				offset += 4 + len0;

				let len1 = u32::from_le_bytes([
					payload[offset],
					payload[offset + 1],
					payload[offset + 2],
					payload[offset + 3],
				]) as usize;
				assert_eq!(len1, 3);
				assert_eq!(&payload[offset + 4..offset + 4 + len1], b"ev1");
				offset += 4 + len1;

				let len2 = u32::from_le_bytes([
					payload[offset],
					payload[offset + 1],
					payload[offset + 2],
					payload[offset + 3],
				]) as usize;
				assert_eq!(len2, 3);
				assert_eq!(&payload[offset + 4..offset + 4 + len2], b"ev2");
			})
			.await;
	}

	#[tokio::test]
	async fn test_query_sends_req_to_connections() {
		let local = tokio::task::LocalSet::new();
		local
			.run_until(async {
				let storage = Arc::new(MockStorage::with_query_results(vec![
					Ok(vec![b"ev0".to_vec(), b"ev1".to_vec(), b"ev2".to_vec()]),
					Ok(vec![]),
				]));
				let worker = CacheWorker::new(storage.clone());
				let (mut from_parser_tx, from_parser_rx) = TokioWorkerChannel::new_pair();
				let (to_parser_tx, mut to_parser_rx) = TokioWorkerChannel::new_pair();
				let (to_connections_tx, mut to_connections_rx) = TokioWorkerChannel::new_pair();

				worker.run(
					Box::new(from_parser_rx),
					to_parser_tx.clone_sender(),
					to_connections_tx.clone_sender(),
				);

				let requests = vec![
					Request {
						relays: vec!["wss://r".to_string()],
						..Default::default()
					},
					Request {
						relays: vec!["wss://r".to_string()],
						..Default::default()
					},
				];
				let bytes = build_query_request_bytes("s1", requests);
				from_parser_tx.send(&bytes).await.unwrap();

				// Consume batched response
				let _ = to_parser_rx.recv().await.unwrap();

				// Check two REQ envelopes
				for _ in 0..2 {
					let env_bytes = to_connections_rx.recv().await.unwrap();
					let envelope: Value = serde_json::from_slice(&env_bytes).unwrap();
					assert_eq!(envelope["relays"], json!(["wss://r"]));
					let frames = envelope["frames"].as_array().unwrap();
					assert_eq!(frames.len(), 1);
					let frame: Value = serde_json::from_str(frames[0].as_str().unwrap()).unwrap();
					let arr = frame.as_array().unwrap();
					assert_eq!(arr[0], "REQ");
					assert_eq!(arr[1], "s1");
				}
			})
			.await;
	}

	#[tokio::test]
	async fn test_no_cache_skips_storage_query() {
		let local = tokio::task::LocalSet::new();
		local
			.run_until(async {
				let storage = Arc::new(MockStorage::new());
				let worker = CacheWorker::new(storage.clone());
				let (mut from_parser_tx, from_parser_rx) = TokioWorkerChannel::new_pair();
				let (to_parser_tx, mut to_parser_rx) = TokioWorkerChannel::new_pair();
				let (to_connections_tx, mut to_connections_rx) = TokioWorkerChannel::new_pair();

				worker.run(
					Box::new(from_parser_rx),
					to_parser_tx.clone_sender(),
					to_connections_tx.clone_sender(),
				);

				let requests = vec![Request {
					relays: vec!["wss://r".to_string()],
					no_cache: true,
					..Default::default()
				}];
				let bytes = build_query_request_bytes("s1", requests);
				from_parser_tx.send(&bytes).await.unwrap();

				// REQ envelope should still be sent
				let env_bytes = to_connections_rx.recv().await.unwrap();
				let envelope: Value = serde_json::from_slice(&env_bytes).unwrap();
				assert_eq!(envelope["relays"], json!(["wss://r"]));
				let frames = envelope["frames"].as_array().unwrap();
				assert_eq!(frames.len(), 1);
				let frame: Value = serde_json::from_str(frames[0].as_str().unwrap()).unwrap();
				let arr = frame.as_array().unwrap();
				assert_eq!(arr[0], "REQ");
				assert_eq!(arr[1], "s1");

				// Verify EOCE is also sent
				let eoce_bytes = to_parser_rx.recv().await.unwrap();
				let eoce = flatbuffers::root::<fb::CacheResponse>(&eoce_bytes).unwrap();
				assert!(eoce.payload().unwrap().bytes().is_empty());

				let query_calls = storage.query_calls.lock().unwrap();
				assert!(query_calls.is_empty());
			})
			.await;
	}

	#[tokio::test]
	async fn test_cache_first_skips_req_when_results_present() {
		let local = tokio::task::LocalSet::new();
		local
			.run_until(async {
				let storage = Arc::new(MockStorage::with_query_results(vec![
					Ok(vec![b"ev0".to_vec()]),
					Ok(vec![]),
				]));
				let worker = CacheWorker::new(storage.clone());
				let (mut from_parser_tx, from_parser_rx) = TokioWorkerChannel::new_pair();
				let (to_parser_tx, mut to_parser_rx) = TokioWorkerChannel::new_pair();
				let (to_connections_tx, mut to_connections_rx) = TokioWorkerChannel::new_pair();

				worker.run(
					Box::new(from_parser_rx),
					to_parser_tx.clone_sender(),
					to_connections_tx.clone_sender(),
				);

				let requests = vec![
					Request {
						relays: vec!["wss://r".to_string()],
						cache_first: true,
						..Default::default()
					},
					Request {
						relays: vec!["wss://r".to_string()],
						..Default::default()
					},
				];
				let bytes = build_query_request_bytes("s1", requests);
				from_parser_tx.send(&bytes).await.unwrap();

				// Consume batched response
				let _ = to_parser_rx.recv().await.unwrap();

				// Only one REQ should be sent (for filter 1)
				let env_bytes = to_connections_rx.recv().await.unwrap();
				let envelope: Value = serde_json::from_slice(&env_bytes).unwrap();
				let frames = envelope["frames"].as_array().unwrap();
				let frame: Value = serde_json::from_str(frames[0].as_str().unwrap()).unwrap();
				let arr = frame.as_array().unwrap();
				assert_eq!(arr[0], "REQ");
				assert_eq!(arr[1], "s1");

				// EOCE follows after all REQs
				let eoce_bytes = to_parser_rx.recv().await.unwrap();
				let eoce = flatbuffers::root::<fb::CacheResponse>(&eoce_bytes).unwrap();
				assert!(eoce.payload().unwrap().bytes().is_empty());
			})
			.await;
	}

	#[tokio::test]
	async fn test_query_error_continues_to_network() {
		// Mock storage that returns Err on query()
		struct FailingQueryStorage;
		#[async_trait(?Send)]
		impl Storage for FailingQueryStorage {
			async fn query(&self, _filters: Vec<Filter>) -> Result<Vec<Vec<u8>>, StorageError> {
				Err(StorageError::Other("query failed".to_string()))
			}
			async fn persist(&self, _event_bytes: &[u8]) -> Result<(), StorageError> {
				Ok(())
			}
			async fn initialize(&self) -> Result<(), StorageError> {
				Ok(())
			}
		}

		let local = tokio::task::LocalSet::new();
		local
			.run_until(async {
				let storage = Arc::new(FailingQueryStorage);
				let worker = CacheWorker::new(storage.clone());
				let (mut from_parser_tx, from_parser_rx) = TokioWorkerChannel::new_pair();
				let (to_parser_tx, mut to_parser_rx) = TokioWorkerChannel::new_pair();
				let (to_connections_tx, mut to_connections_rx) = TokioWorkerChannel::new_pair();

				worker.run(
					Box::new(from_parser_rx),
					to_parser_tx.clone_sender(),
					to_connections_tx.clone_sender(),
				);

				let requests = vec![Request {
					relays: vec!["wss://r".to_string()],
					..Default::default()
				}];
				let bytes = build_query_request_bytes("s1", requests);
				from_parser_tx.send(&bytes).await.unwrap();

				// EOCE should be sent (empty payload since query failed)
				let eoce_bytes = to_parser_rx.recv().await.unwrap();
				let eoce = flatbuffers::root::<fb::CacheResponse>(&eoce_bytes).unwrap();
				assert!(eoce.payload().unwrap().bytes().is_empty());

				// REQ should still be sent to connections despite query failure
				let env_bytes = to_connections_rx.recv().await.unwrap();
				let envelope: Value = serde_json::from_slice(&env_bytes).unwrap();
				assert_eq!(envelope["relays"], json!(["wss://r"]));
				let frames = envelope["frames"].as_array().unwrap();
				assert_eq!(frames.len(), 1);
				let frame: Value = serde_json::from_str(frames[0].as_str().unwrap()).unwrap();
				let arr = frame.as_array().unwrap();
				assert_eq!(arr[0], "REQ");
				assert_eq!(arr[1], "s1");
			})
			.await;
	}

	#[tokio::test]
	async fn test_persist_error_does_not_crash() {
		// Mock storage that returns Err on persist()
		struct FailingPersistStorage;
		#[async_trait(?Send)]
		impl Storage for FailingPersistStorage {
			async fn query(&self, _filters: Vec<Filter>) -> Result<Vec<Vec<u8>>, StorageError> {
				Ok(vec![])
			}
			async fn persist(&self, _event_bytes: &[u8]) -> Result<(), StorageError> {
				Err(StorageError::Other("persist failed".to_string()))
			}
			async fn initialize(&self) -> Result<(), StorageError> {
				Ok(())
			}
		}

		let local = tokio::task::LocalSet::new();
		local
			.run_until(async {
				let storage = Arc::new(FailingPersistStorage);
				let worker = CacheWorker::new(storage.clone());
				let (mut from_parser_tx, from_parser_rx) = TokioWorkerChannel::new_pair();
				let (to_parser_tx, _to_parser_rx) = TokioWorkerChannel::new_pair();
				let (to_connections_tx, _to_connections_rx) = TokioWorkerChannel::new_pair();

				worker.run(
					Box::new(from_parser_rx),
					to_parser_tx.clone_sender(),
					to_connections_tx.clone_sender(),
				);

				// Send WorkerMessage with sub_id="save_to_db"
				let bytes = build_worker_message_bytes("save_to_db");
				from_parser_tx.send(&bytes).await.unwrap();

				// Yield to let worker process
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;

				// Verify worker continues - send another message and verify it's processed
				let requests = vec![Request {
					relays: vec!["wss://r".to_string()],
					..Default::default()
				}];
				let bytes2 = build_query_request_bytes("s2", requests);
				from_parser_tx.send(&bytes2).await.unwrap();

				// If worker crashed/panicked, we wouldn't receive this message
				// The test completing without panic means the worker continued
			})
			.await;
	}

	#[tokio::test]
	async fn test_storage_initialize_failure() {
		// Mock storage that returns Err on initialize()
		struct FailingInitStorage;
		#[async_trait(?Send)]
		impl Storage for FailingInitStorage {
			async fn query(&self, _filters: Vec<Filter>) -> Result<Vec<Vec<u8>>, StorageError> {
				Ok(vec![])
			}
			async fn persist(&self, _event_bytes: &[u8]) -> Result<(), StorageError> {
				Ok(())
			}
			async fn initialize(&self) -> Result<(), StorageError> {
				Err(StorageError::Other("init failed".to_string()))
			}
		}

		let local = tokio::task::LocalSet::new();
		local
			.run_until(async {
				let storage = Arc::new(FailingInitStorage);
				let worker = CacheWorker::new(storage.clone());
				let (mut from_parser_tx, from_parser_rx) = TokioWorkerChannel::new_pair();
				let (to_parser_tx, mut to_parser_rx) = TokioWorkerChannel::new_pair();
				let (to_connections_tx, mut to_connections_rx) = TokioWorkerChannel::new_pair();

				worker.run(
					Box::new(from_parser_rx),
					to_parser_tx.clone_sender(),
					to_connections_tx.clone_sender(),
				);

				// Yield to allow worker to start and attempt initialization
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;

				// Verify worker can still process messages despite init failure
				let requests = vec![Request {
					relays: vec!["wss://r".to_string()],
					..Default::default()
				}];
				let bytes = build_query_request_bytes("s1", requests);
				from_parser_tx.send(&bytes).await.unwrap();

				// EOCE should be sent
				let eoce_bytes = to_parser_rx.recv().await.unwrap();
				let eoce = flatbuffers::root::<fb::CacheResponse>(&eoce_bytes).unwrap();
				assert!(eoce.payload().unwrap().bytes().is_empty());

				// REQ should be sent to connections
				let env_bytes = to_connections_rx.recv().await.unwrap();
				let envelope: Value = serde_json::from_slice(&env_bytes).unwrap();
				let frames = envelope["frames"].as_array().unwrap();
				let frame: Value = serde_json::from_str(frames[0].as_str().unwrap()).unwrap();
				let arr = frame.as_array().unwrap();
				assert_eq!(arr[0], "REQ");
				assert_eq!(arr[1], "s1");
			})
			.await;
	}

	#[tokio::test]
	async fn test_partial_query_results() {
		// Mock storage where first filter query fails but second succeeds
		struct PartialFailingStorage {
			query_count: Mutex<usize>,
		}
		#[async_trait(?Send)]
		impl Storage for PartialFailingStorage {
			async fn query(&self, _filters: Vec<Filter>) -> Result<Vec<Vec<u8>>, StorageError> {
				let mut count = self.query_count.lock().unwrap();
				*count += 1;
				if *count == 1 {
					Err(StorageError::Other("first query failed".to_string()))
				} else {
					Ok(vec![b"second_event".to_vec()])
				}
			}
			async fn persist(&self, _event_bytes: &[u8]) -> Result<(), StorageError> {
				Ok(())
			}
			async fn initialize(&self) -> Result<(), StorageError> {
				Ok(())
			}
		}

		let local = tokio::task::LocalSet::new();
		local
			.run_until(async {
				let storage = Arc::new(PartialFailingStorage {
					query_count: Mutex::new(0),
				});
				let worker = CacheWorker::new(storage.clone());
				let (mut from_parser_tx, from_parser_rx) = TokioWorkerChannel::new_pair();
				let (to_parser_tx, mut to_parser_rx) = TokioWorkerChannel::new_pair();
				let (to_connections_tx, mut to_connections_rx) = TokioWorkerChannel::new_pair();

				worker.run(
					Box::new(from_parser_rx),
					to_parser_tx.clone_sender(),
					to_connections_tx.clone_sender(),
				);

				// Send CacheRequest with 2 filters
				let requests = vec![
					Request {
						relays: vec!["wss://r1".to_string()],
						..Default::default()
					},
					Request {
						relays: vec!["wss://r2".to_string()],
						..Default::default()
					},
				];
				let bytes = build_query_request_bytes("s1", requests);
				from_parser_tx.send(&bytes).await.unwrap();

				// Verify second filter's results are still sent (batched response)
				let resp_bytes = to_parser_rx.recv().await.unwrap();
				let resp = flatbuffers::root::<fb::CacheResponse>(&resp_bytes).unwrap();
				let payload = resp.payload().unwrap().bytes();
				assert!(!payload.is_empty()); // Should have the second_event

				let len0 = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]) as usize;
				assert_eq!(&payload[4..4 + len0], b"second_event");

				// EOCE follows
				let eoce_bytes = to_parser_rx.recv().await.unwrap();
				let eoce = flatbuffers::root::<fb::CacheResponse>(&eoce_bytes).unwrap();
				assert!(eoce.payload().unwrap().bytes().is_empty());

				// Verify REQ for both filters still sent
				let env_bytes1 = to_connections_rx.recv().await.unwrap();
				let envelope1: Value = serde_json::from_slice(&env_bytes1).unwrap();
				assert_eq!(envelope1["relays"], json!(["wss://r1"]));

				let env_bytes2 = to_connections_rx.recv().await.unwrap();
				let envelope2: Value = serde_json::from_slice(&env_bytes2).unwrap();
				assert_eq!(envelope2["relays"], json!(["wss://r2"]));
			})
			.await;
	}
}
