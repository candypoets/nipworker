use std::sync::Arc;
use futures::channel::mpsc;
use tracing::info;

use crate::channel::{ChannelPort, WorkerChannel, WorkerChannelSender};
#[cfg(not(target_arch = "wasm32"))]
use crate::channel::TokioWorkerChannel;
#[cfg(target_arch = "wasm32")]
use crate::channel::WasmWorkerChannel;
use crate::crypto_client::CryptoClient;
use crate::generated::nostr::fb;
use crate::nostr_error::{NostrError, NostrResult};
use crate::parser::Parser;
use crate::spawn::spawn_worker;
use crate::traits::{RelayTransport, Signer, Storage};
use crate::types::network::Request;
use crate::types::nostr::Template;
#[cfg(not(target_arch = "wasm32"))]
use crate::worker::cache_worker::CacheWorker;
#[cfg(not(target_arch = "wasm32"))]
use crate::worker::connections_worker::ConnectionsWorker;
#[cfg(not(target_arch = "wasm32"))]
use crate::worker::crypto_worker::CryptoWorker;
#[cfg(not(target_arch = "wasm32"))]
use crate::worker::parser_worker::ParserWorker;

/// NostrEngine is the Rust equivalent of the TypeScript NostrManager / Orchestrator.
///
/// On native: it creates internal WorkerChannel pairs, spawns the 4 workers,
/// and runs a main loop that forwards events to `event_sink`.
///
/// On WASM (browser): it runs on the main thread and communicates with the
/// externally-spawned Web Workers via WorkerChannels backed by MessagePorts.
pub struct NostrEngine {
	parser_tx: Box<dyn WorkerChannelSender>,
	crypto_tx: Box<dyn WorkerChannelSender>,
	event_sink: mpsc::Sender<(String, Vec<u8>)>,
}

impl NostrEngine {
	/// Native constructor: NostrEngine is the orchestrator and spawns all workers internally.
	#[cfg(not(target_arch = "wasm32"))]
	pub fn new(
		transport: Arc<dyn RelayTransport>,
		storage: Arc<dyn Storage>,
		signer: Arc<dyn Signer>,
		event_sink: mpsc::Sender<(String, Vec<u8>)>,
	) -> Self {
		info!("[NostrEngine] Initializing...");

		// Bidirectional pairs: one end stays in the engine, the other goes to the worker.
		let (engine_parser_ch, parser_engine_ch) = TokioWorkerChannel::new_pair();
		let (parser_conn_ch, conn_parser_ch) = TokioWorkerChannel::new_pair();
		let (parser_cache_ch, cache_parser_ch) = TokioWorkerChannel::new_pair();
		let (parser_crypto_ch, crypto_parser_ch) = TokioWorkerChannel::new_pair();
		let (engine_crypto_ch, crypto_engine_ch) = TokioWorkerChannel::new_pair();

		let engine_parser_tx = engine_parser_ch.clone_sender();
		let engine_crypto_tx = engine_crypto_ch.clone_sender();
		let conn_parser_tx = conn_parser_ch.clone_sender();
		let cache_parser_tx = cache_parser_ch.clone_sender();
		let crypto_engine_tx = crypto_engine_ch.clone_sender();
		let crypto_parser_tx = crypto_parser_ch.clone_sender();

		let (parser_main_tx, mut parser_main_rx) =
			tokio::sync::mpsc::unbounded_channel::<(String, Vec<u8>)>();

		let crypto_client = Arc::new(CryptoClient::new());
		let parser = Arc::new(Parser::new(crypto_client.clone()));

		let parser_worker = ParserWorker::new(
			parser.clone(),
			Arc::new(ChannelPort::new(parser_cache_ch.clone_sender())),
			parser_main_tx,
			crypto_client.clone(),
		);
		parser_worker.run(
			Box::new(parser_engine_ch),
			Box::new(conn_parser_ch),
			Box::new(cache_parser_ch),
		);

		let connections_worker = ConnectionsWorker::new(transport);
		connections_worker.run(Box::new(parser_conn_ch), conn_parser_tx);

		let cache_worker = CacheWorker::new(storage);
		cache_worker.run(Box::new(parser_cache_ch), cache_parser_tx);

		let crypto_worker = CryptoWorker::new(signer);
		crypto_worker.run(
			Box::new(crypto_engine_ch),
			Box::new(parser_crypto_ch),
			crypto_engine_tx,
			crypto_parser_tx,
		);

		let event_sink_clone = event_sink.clone();
		spawn_worker(async move {
			loop {
				tokio::select! {
					some = parser_main_rx.recv() => {
						match some {
							Some((sub_id, bytes)) => {
								if let Err(e) = event_sink_clone.clone().try_send((sub_id, bytes)) {
									tracing::warn!("Failed to forward parser event to sink: {}", e);
								}
							}
							None => break,
						}
					}
				}
			}
			info!("[NostrEngine] main loop exiting");
		});

		Self {
			parser_tx: engine_parser_tx,
			crypto_tx: engine_crypto_tx,
			event_sink,
		}
	}

	/// WASM constructor: NostrEngine runs on the main thread and receives
	/// WorkerChannels (backed by MessagePorts) for each worker link.
	#[cfg(target_arch = "wasm32")]
	pub fn new(
		mut parser_main: Box<dyn WorkerChannel>,
		mut connections_main: Box<dyn WorkerChannel>,
		mut crypto_main: Box<dyn WorkerChannel>,
		event_sink: mpsc::Sender<(String, Vec<u8>)>,
	) -> Self {
		info!("[NostrEngine] Initializing (WASM)...");

		let parser_tx = parser_main.clone_sender();
		let crypto_tx = crypto_main.clone_sender();

		spawn_worker(async move {
			use futures::FutureExt;
			let mut parser_main = parser_main;
			let mut connections_main = connections_main;
			let mut crypto_main = crypto_main;
			loop {
				let mut parser_fut = parser_main.recv().fuse();
				let mut conn_fut = connections_main.recv().fuse();
				let mut crypto_fut = crypto_main.recv().fuse();
				futures::select! {
					msg = parser_fut => {
						match msg {
							Ok(bytes) => {
								if let Err(e) = event_sink.clone().try_send(("parser".to_string(), bytes)) {
									tracing::warn!("Failed to forward parser event: {}", e);
								}
							}
							Err(_) => break,
						}
					}
					msg = conn_fut => {
						match msg {
							Ok(bytes) => {
								if let Err(e) = event_sink.clone().try_send(("connections".to_string(), bytes)) {
									tracing::warn!("Failed to forward connections status: {}", e);
								}
							}
							Err(_) => break,
						}
					}
					msg = crypto_fut => {
						match msg {
							Ok(bytes) => {
								if let Err(e) = event_sink.clone().try_send(("crypto".to_string(), bytes)) {
									tracing::warn!("Failed to forward crypto response: {}", e);
								}
							}
							Err(_) => break,
						}
					}
				}
			}
			info!("[NostrEngine] WASM main loop exiting");
		});

		Self {
			parser_tx,
			crypto_tx,
			event_sink: futures::channel::mpsc::channel(1).0,
		}
	}

	/// Deserialize a FlatBuffers MainMessage and dispatch to the appropriate worker.
	pub async fn handle_message(&self, bytes: &[u8]) -> NostrResult<()> {
		let main_message = flatbuffers::root::<fb::MainMessage>(bytes)
			.map_err(|e| NostrError::Parse(format!("Failed to decode FlatBuffer: {:?}", e)))?;

		match main_message.content_type() {
			fb::MainContent::Subscribe
			| fb::MainContent::Unsubscribe
			| fb::MainContent::Publish => {
				self.parser_tx
					.send(bytes)
					.map_err(|e| NostrError::Other(format!("Failed to send to parser: {}", e)))?;
			}
			fb::MainContent::SignEvent
			| fb::MainContent::SetSigner
			| fb::MainContent::GetPublicKey => {
				self.crypto_tx
					.send(bytes)
					.map_err(|e| NostrError::Other(format!("Failed to send to crypto: {}", e)))?;
			}
			_ => {
				return Err(NostrError::Parse("Empty or unknown message content".to_string()));
			}
		}
		Ok(())
	}

	pub async fn subscribe(
		&self,
		subscription_id: String,
		requests: Vec<Request>,
	) -> NostrResult<()> {
		let mut builder = flatbuffers::FlatBufferBuilder::new();
		let sub_id_offset = builder.create_string(&subscription_id);
		let request_offsets: Vec<_> = requests
			.iter()
			.map(|r| r.build_flatbuffer(&mut builder))
			.collect();
		let requests_vec = builder.create_vector(&request_offsets);

		let subscribe = fb::Subscribe::create(
			&mut builder,
			&fb::SubscribeArgs {
				subscription_id: Some(sub_id_offset),
				requests: Some(requests_vec),
				config: None,
			},
		);
		let main_msg = fb::MainMessage::create(
			&mut builder,
			&fb::MainMessageArgs {
				content_type: fb::MainContent::Subscribe,
				content: Some(subscribe.as_union_value()),
			},
		);
		builder.finish(main_msg, None);
		let bytes = builder.finished_data();

		self.parser_tx
			.send(bytes)
			.map_err(|e| NostrError::Other(format!("Failed to send subscribe: {}", e)))?;
		Ok(())
	}

	pub async fn unsubscribe(&self, subscription_id: String) -> NostrResult<()> {
		let mut builder = flatbuffers::FlatBufferBuilder::new();
		let sub_id_offset = builder.create_string(&subscription_id);
		let unsubscribe = fb::Unsubscribe::create(
			&mut builder,
			&fb::UnsubscribeArgs {
				subscription_id: Some(sub_id_offset),
			},
		);
		let main_msg = fb::MainMessage::create(
			&mut builder,
			&fb::MainMessageArgs {
				content_type: fb::MainContent::Unsubscribe,
				content: Some(unsubscribe.as_union_value()),
			},
		);
		builder.finish(main_msg, None);
		let bytes = builder.finished_data();

		self.parser_tx
			.send(bytes)
			.map_err(|e| NostrError::Other(format!("Failed to send unsubscribe: {}", e)))?;
		Ok(())
	}

	pub async fn publish(
		&self,
		publish_id: String,
		template: &Template,
		relays: Vec<String>,
		optimistic_subids: Vec<String>,
	) -> NostrResult<()> {
		let mut builder = flatbuffers::FlatBufferBuilder::new();
		let publish_id_offset = builder.create_string(&publish_id);
		let template_offset = template.build_flatbuffer(&mut builder);
		let relay_offsets: Vec<_> = relays.iter().map(|r| builder.create_string(r)).collect();
		let relay_vec = builder.create_vector(&relay_offsets);
		let opt_subid_offsets: Vec<_> =
			optimistic_subids.iter().map(|s| builder.create_string(s)).collect();
		let opt_subid_vec = builder.create_vector(&opt_subid_offsets);

		let publish = fb::Publish::create(
			&mut builder,
			&fb::PublishArgs {
				publish_id: Some(publish_id_offset),
				template: Some(template_offset),
				relays: Some(relay_vec),
				optimistic_subids: Some(opt_subid_vec),
			},
		);
		let main_msg = fb::MainMessage::create(
			&mut builder,
			&fb::MainMessageArgs {
				content_type: fb::MainContent::Publish,
				content: Some(publish.as_union_value()),
			},
		);
		builder.finish(main_msg, None);
		let bytes = builder.finished_data();

		self.parser_tx
			.send(bytes)
			.map_err(|e| NostrError::Other(format!("Failed to send publish: {}", e)))?;
		Ok(())
	}
}
