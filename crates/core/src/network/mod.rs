pub mod interfaces;
pub mod publish;
pub mod subscription;

use std::sync::{Arc, Mutex};

use futures::channel::mpsc;
use rustc_hash::FxHashMap;
use tracing::{info, warn};

use crate::generated::nostr::fb;
use crate::network::publish::PublishManager;
use crate::network::subscription::SubscriptionManager;
use crate::nostr_error::{NostrError, NostrResult};
use crate::parser::Parser;
use crate::pipeline::Pipeline;
use crate::port::Port;
use crate::traits::{RelayTransport, Storage};
use crate::types::network::Request;
use crate::types::nostr::Template;
use crate::types::Event;

/// Dummy port stub for cache persistence in the default pipeline.
/// In a full implementation this would bridge to `Arc<dyn Storage>`.
struct DummyPort;

impl Port for DummyPort {
	fn send(&self, _bytes: &[u8]) -> Result<(), String> {
		Ok(())
	}
}

struct NetworkManagerInner {
	_transport: Arc<dyn RelayTransport>,
	_storage: Arc<dyn Storage>,
	parser: Arc<Parser>,
	event_sink: mpsc::Sender<(String, Vec<u8>)>,
	subscriptions: Mutex<FxHashMap<String, Arc<futures::lock::Mutex<Pipeline>>>>,
	publish_manager: PublishManager,
}

/// Platform-agnostic network manager that stores subscriptions and routes
/// incoming transport events through the parser pipeline.
#[derive(Clone)]
pub struct NetworkManager {
	inner: Arc<NetworkManagerInner>,
}

impl NetworkManager {
	pub fn new(
		transport: Arc<dyn RelayTransport>,
		storage: Arc<dyn Storage>,
		parser: Arc<Parser>,
		event_sink: mpsc::Sender<(String, Vec<u8>)>,
	) -> Self {
		let publish_manager = PublishManager::new(parser.clone());
		Self {
			inner: Arc::new(NetworkManagerInner {
				_transport: transport,
				_storage: storage,
				parser,
				event_sink,
				subscriptions: Mutex::new(FxHashMap::default()),
				publish_manager,
			}),
		}
	}

	/// Open a subscription using the pipeline configuration supplied in the
	/// FlatBuffers `SubscriptionConfig`.
	pub async fn open_subscription(
		&self,
		subscription_id: String,
		requests: Vec<Request>,
		config: &fb::SubscriptionConfig<'_>,
	) -> NostrResult<()> {
		if self.inner.subscriptions.lock().unwrap().contains_key(&subscription_id) {
			return Ok(());
		}

		let to_cache: Arc<dyn Port> = Arc::new(DummyPort);
		let subscription_manager =
			SubscriptionManager::new(self.inner.parser.clone());
		let pipeline = subscription_manager
			.process_subscription(&subscription_id, to_cache, requests, config)
			.await?;

		// TODO: wire transport.on_message callbacks for each relay URL so
		// incoming relay frames are sent through the pipeline and back to
		// event_sink via process_event().
		let _ = &self.inner._transport;

		self.inner
			.subscriptions
			.lock().unwrap()
			.insert(subscription_id, Arc::new(futures::lock::Mutex::new(pipeline)));
		Ok(())
	}

	/// Open a subscription with the default parse + save + serialize pipeline.
	pub async fn open_subscription_with_default(
		&self,
		subscription_id: String,
		_requests: Vec<Request>,
	) -> NostrResult<()> {
		if self.inner.subscriptions.lock().unwrap().contains_key(&subscription_id) {
			return Ok(());
		}

		let to_cache: Arc<dyn Port> = Arc::new(DummyPort);
		let pipeline = Pipeline::default(
			self.inner.parser.clone(),
			to_cache,
			subscription_id.clone(),
		)
		.map_err(|e| NostrError::Pipeline(format!("Failed to create pipeline: {}", e)))?;

		self.inner
			.subscriptions
			.lock().unwrap()
			.insert(subscription_id, Arc::new(futures::lock::Mutex::new(pipeline)));
		Ok(())
	}

	pub async fn close_subscription(&self, subscription_id: String) -> NostrResult<()> {
		self.inner.subscriptions.lock().unwrap().remove(&subscription_id);
		Ok(())
	}

	pub async fn publish_event(
		&self,
		publish_id: String,
		template: &Template,
		_relays: &Vec<String>,
		_optimistic_subids: Vec<String>,
	) -> NostrResult<Event> {
		info!("[NetworkManager] Publishing event {}", publish_id);
		self.inner.publish_manager.publish_event(publish_id, template).await
	}

	/// Run a single raw JSON event through the subscription's pipeline and
	/// forward any resulting serialized bytes to `event_sink`.
	pub async fn process_event(&self, sub_id: &str, event_json: &str) -> NostrResult<()> {
		let subs = self.inner.subscriptions.lock().unwrap();
		let Some(pipeline_arc) = subs.get(sub_id).cloned() else {
			return Ok(());
		};
		drop(subs);

		let mut pipeline = pipeline_arc.lock().await;
		match pipeline.process(event_json).await {
			Ok(Some(output)) => {
				let mut sink = self.inner.event_sink.clone();
				if let Err(e) = sink.try_send((sub_id.to_string(), output)) {
					warn!("Failed to send pipeline output to event_sink: {}", e);
				}
			}
			Ok(None) => {}
			Err(e) => {
				warn!("Pipeline process failed for sub {}: {}", sub_id, e);
			}
		}
		Ok(())
	}
}
