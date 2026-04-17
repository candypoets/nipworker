use std::sync::Arc;
use async_trait::async_trait;
use futures::channel::mpsc;
use tracing::info;

use crate::crypto_client::CryptoClient;
use crate::generated::nostr::fb;
use crate::network::NetworkManager;
use crate::nostr_error::{NostrError, NostrResult};
use crate::parser::Parser;
use crate::traits::{RelayTransport, Signer, SignerError, Storage};
use crate::types::network::Request;
use crate::types::nostr::{Event, Template};

#[cfg(feature = "crypto")]
use crate::crypto::signers::PrivateKeySigner;

/// Central Nostr engine that wires together transport, storage, signer,
/// parser, and pipeline using platform-agnostic trait abstractions.
pub struct NostrEngine {
	network_manager: NetworkManager,
	_crypto_client: Arc<CryptoClient>,
	_parser: Arc<Parser>,
	signer: Arc<async_lock::RwLock<Arc<dyn Signer>>>,
	event_sink: mpsc::Sender<(String, Vec<u8>)>,
}

impl NostrEngine {
	pub fn new(
		transport: Arc<dyn RelayTransport>,
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
			signer: Arc::new(async_lock::RwLock::new(signer)),
			event_sink,
		}
	}

	/// Replace the current signer.
	pub async fn set_signer(&self, signer: Arc<dyn Signer>) {
		*self.signer.write().await = signer;
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
				let sign_event = main_message.content_as_sign_event().ok_or_else(|| {
					NostrError::Parse("Invalid SignEvent message".to_string())
				})?;
				let template = Template::from_flatbuffer(&sign_event.template());
				let template_json = template.to_json();
				let signed_json = self
					.signer
					.read()
					.await
					.sign_event(&template_json)
					.await
					.map_err(|e| NostrError::Crypto(format!("SignEvent error: {}", e)))?;
				let event = Event::from_json(&signed_json)
					.map_err(|e| NostrError::Parse(format!("Failed to parse signed event: {}", e)))?;

				let mut builder = flatbuffers::FlatBufferBuilder::new();
				let event_offset = event.build_flatbuffer(&mut builder);
				let signed_event_payload = fb::SignedEvent::create(
					&mut builder,
					&fb::SignedEventArgs {
						event: Some(event_offset),
					},
				);
				let worker_msg = fb::WorkerMessage::create(
					&mut builder,
					&fb::WorkerMessageArgs {
						sub_id: None,
						url: None,
						type_: fb::MessageType::SignedEvent,
						content_type: fb::Message::SignedEvent,
						content: Some(signed_event_payload.as_union_value()),
					},
				);
				builder.finish(worker_msg, None);
				let data = builder.finished_data().to_vec();

				self.event_sink
					.clone()
					.try_send(("".to_string(), data))
					.map_err(|e| NostrError::Other(format!("Failed to send signed event: {}", e)))?;
				Ok(())
			}
			fb::MainContent::SetSigner => {
				let set_signer = main_message.content_as_set_signer().ok_or_else(|| {
					NostrError::Parse("Invalid SetSigner message".to_string())
				})?;
				match set_signer.signer_type_type() {
					fb::SignerType::PrivateKey => {
						let pk = set_signer.signer_type_as_private_key().ok_or_else(|| {
							NostrError::Parse("Invalid PrivateKey signer data".to_string())
						})?;
						let private_key = pk.private_key().to_string();
						#[cfg(feature = "crypto")]
						{
							let signer = PrivateKeySigner::new(&private_key)
								.map_err(|e| NostrError::Crypto(format!("Failed to create signer: {}", e)))?;
							self.set_signer(Arc::new(signer)).await;
							Ok(())
						}
						#[cfg(not(feature = "crypto"))]
						{
							Err(NostrError::Crypto(
								"PrivateKey signer requires crypto feature".to_string(),
							))
						}
					}
					_ => Err(NostrError::Parse("Unsupported signer type".to_string())),
				}
			}
			fb::MainContent::GetPublicKey => {
				let pubkey = self
					.signer
					.read()
					.await
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
