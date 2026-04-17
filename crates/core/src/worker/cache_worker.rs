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

					for i in 0..reqs.len() {
						let fb_req = reqs.get(i);
						let request = Request::from_flatbuffer(&fb_req);
						let filter = match request.to_filter() {
							Ok(f) => f,
							Err(e) => {
								warn!("[CacheWorker] failed to convert request to filter: {}", e);
								continue;
							}
						};

						match self._storage.query(vec![filter]).await {
							Ok(events) => all_cached_events.extend(events),
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
