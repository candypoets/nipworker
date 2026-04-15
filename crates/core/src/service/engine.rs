use std::sync::Arc;
use futures::channel::mpsc;
use tracing::info;

use crate::crypto_client::CryptoClient;
use crate::generated::nostr::fb;
use crate::network::NetworkManager;
use crate::nostr_error::{NostrError, NostrResult};
use crate::parser::Parser;
use crate::traits::{Signer, Storage, Transport};
use crate::types::network::Request;
use crate::types::nostr::Template;

/// Central Nostr engine that wires together transport, storage, signer,
/// parser, and pipeline using platform-agnostic trait abstractions.
pub struct NostrEngine {
	network_manager: NetworkManager,
	_crypto_client: Arc<CryptoClient>,
	_parser: Arc<Parser>,
	signer: Arc<dyn Signer>,
	event_sink: mpsc::Sender<(String, Vec<u8>)>,
}

impl NostrEngine {
	pub fn new(
		transport: Arc<dyn Transport>,
		storage: Arc<dyn Storage>,
		signer: Arc<dyn Signer>,
		event_sink: mpsc::Sender<(String, Vec<u8>)>,
	) -> Self {
		info!("[NostrEngine] Initializing...");

		let crypto_client = Arc::new(CryptoClient::new());
		let parser = Arc::new(Parser::new(crypto_client.clone()));

		let network_manager = NetworkManager::new(
			transport,
			storage,
			parser.clone(),
			crypto_client.clone(),
			event_sink.clone(),
		);

		Self {
			network_manager,
			_crypto_client: crypto_client,
			_parser: parser,
			signer,
			event_sink,
		}
	}

	/// Deserialize a FlatBuffers MainMessage and dispatch to the appropriate
	/// internal service.
	pub async fn handle_message(&self, bytes: &[u8]) -> NostrResult<()> {
		let main_message = flatbuffers::root::<fb::MainMessage>(bytes)
			.map_err(|e| NostrError::Parse(format!("Failed to decode FlatBuffer: {:?}", e)))?;

		match main_message.content_type() {
			fb::MainContent::Subscribe => {
				let subscribe = main_message.content_as_subscribe().ok_or_else(|| {
					NostrError::Parse("Invalid Subscribe message".to_string())
				})?;
				let subscription_id = subscribe.subscription_id().to_string();
				let requests: Vec<Request> = (0..subscribe.requests().len())
					.map(|i| Request::from_flatbuffer(&subscribe.requests().get(i)))
					.collect();
				let config = subscribe.config();
				self.network_manager
					.open_subscription(subscription_id, requests, &config)
					.await
			}
			fb::MainContent::Unsubscribe => {
				let unsubscribe = main_message.content_as_unsubscribe().ok_or_else(|| {
					NostrError::Parse("Invalid Unsubscribe message".to_string())
				})?;
				let subscription_id = unsubscribe.subscription_id().to_string();
				self.network_manager.close_subscription(subscription_id).await
			}
			fb::MainContent::Publish => {
				let publish = main_message.content_as_publish().ok_or_else(|| {
					NostrError::Parse("Invalid Publish message".to_string())
				})?;
				let publish_id = publish.publish_id().to_string();
				let template = Template::from_flatbuffer(&publish.template());
				let relays: Vec<String> = (0..publish.relays().len())
					.map(|i| publish.relays().get(i).to_string())
					.collect();
				let optimistic_subids: Vec<String> = publish
					.optimistic_subids()
					.map(|ids| (0..ids.len()).map(|i| ids.get(i).to_string()).collect())
					.unwrap_or_default();
				self.network_manager
					.publish_event(publish_id, &template, &relays, optimistic_subids)
					.await?;
				Ok(())
			}
			fb::MainContent::SignEvent => {
				// Stub: sign-event flow not yet fully ported to trait abstractions.
				Ok(())
			}
			fb::MainContent::SetSigner => {
				// Stub: signer is now provided via constructor.
				Ok(())
			}
			fb::MainContent::GetPublicKey => {
				let pubkey = self
					.signer
					.get_public_key()
					.await
					.map_err(|e| NostrError::Crypto(format!("Signer error: {}", e)))?;

				let mut builder = flatbuffers::FlatBufferBuilder::new();
				let pubkey_offset = builder.create_string(&pubkey);
				let pubkey_payload = fb::Pubkey::create(
					&mut builder,
					&fb::PubkeyArgs {
						pubkey: Some(pubkey_offset),
					},
				);
				let signer_msg = fb::WorkerMessage::create(
					&mut builder,
					&fb::WorkerMessageArgs {
						sub_id: None,
						url: None,
						type_: fb::MessageType::Pubkey,
						content_type: fb::Message::Pubkey,
						content: Some(pubkey_payload.as_union_value()),
					},
				);
				builder.finish(signer_msg, None);
				let data = builder.finished_data().to_vec();

				self.event_sink
					.clone()
					.try_send(("".to_string(), data))
					.map_err(|e| NostrError::Other(format!("Failed to send pubkey: {}", e)))?;
				Ok(())
			}
			_ => Err(NostrError::Parse("Empty or unknown message content".to_string())),
		}
	}

	/// Direct convenience method to open a subscription with a default pipeline.
	pub async fn subscribe(
		&self,
		subscription_id: String,
		requests: Vec<Request>,
	) -> NostrResult<()> {
		self.network_manager
			.open_subscription_with_default(subscription_id, requests)
			.await
	}

	/// Direct convenience method to close a subscription.
	pub async fn unsubscribe(&self, subscription_id: String) -> NostrResult<()> {
		self.network_manager.close_subscription(subscription_id).await
	}

	/// Direct convenience method to publish an event.
	pub async fn publish(
		&self,
		publish_id: String,
		template: &Template,
		relays: Vec<String>,
		optimistic_subids: Vec<String>,
	) -> NostrResult<()> {
		self.network_manager
			.publish_event(publish_id, template, &relays, optimistic_subids)
			.await?;
		Ok(())
	}
}
