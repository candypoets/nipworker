pub mod cache_processor;
pub mod interfaces;
pub mod publish;
pub mod subscription;

use crate::generated::nostr::fb;
use crate::nostr::Template;
use crate::parser::Parser;
use crate::relays::ClientMessage;
use crate::types::network::Request;
use crate::utils::sab_ring::WsRings;
use crate::NostrError;
use crate::{db::NostrDB, pipeline::Pipeline};
use js_sys::SharedArrayBuffer;
use rustc_hash::FxHashMap;
use std::sync::{Arc, RwLock};
use tracing::info;

type Result<T> = std::result::Result<T, NostrError>;

struct Sub {
    pipeline: Pipeline,
    buffer: SharedArrayBuffer,
}

pub struct NetworkManager {
    rings: WsRings,
    publish_manager: publish::PublishManager,
    subscription_manager: subscription::SubscriptionManager,
    subscriptions: Arc<RwLock<FxHashMap<String, Sub>>>,
}

impl NetworkManager {
    pub fn new(database: Arc<NostrDB>, parser: Arc<Parser>, rings: WsRings) -> Self {
        let publish_manager = publish::PublishManager::new(database.clone(), parser.clone());

        let subscription_manager =
            subscription::SubscriptionManager::new(database.clone(), parser.clone());

        Self {
            rings,
            publish_manager,
            subscription_manager,
            subscriptions: Arc::new(RwLock::new(FxHashMap::default())),
        }
    }

    pub async fn open_subscription(
        &self,
        subscription_id: String,
        shared_buffer: SharedArrayBuffer,
        requests: &Vec<fb::Request<'_>>,
        config: &fb::SubscriptionConfig<'_>,
    ) -> Result<()> {
        info!("Opening subscription: {}", subscription_id);

        // early bailout if the sub already exist
        if self
            .subscriptions
            .read()
            .unwrap()
            .contains_key(&subscription_id)
        {
            return Ok(());
        }

        let parsed_requests: Vec<Request> = requests
            .iter()
            .map(|request| Request::from_flatbuffer(request))
            .collect();

        let (pipeline, relay_filters) = self
            .subscription_manager
            .process_subscription(
                &subscription_id,
                shared_buffer.clone(),
                parsed_requests,
                config,
            )
            .await?;

        self.subscriptions.write().unwrap().insert(
            subscription_id.clone(),
            Sub {
                pipeline,
                buffer: shared_buffer.clone(),
            },
        );

        // Construct and write one REQ frame per relay group:
        // ["REQ", subscription_id, ...filters]
        for (relay_url, filters) in relay_filters {
            let req_message = ClientMessage::req(subscription_id.clone(), filters);

            let frame = req_message.to_json()?;
            let relays = [relay_url.as_str()];
            let frames = [frame];

            // // Write JSON envelope { relays: [...], frames: [...] } to the inRing.
            // // Use an unsafe mutable borrow to avoid changing struct mutability here.
            let _ = self.rings.write_in_envelope(&relays, &frames);
        }

        Ok(())
    }

    pub async fn close_subscription(&self, subscription_id: String) -> Result<()> {
        self.subscription_manager
            .close_subscription(&subscription_id)
            .await
    }

    pub async fn publish_event(
        &self,
        publish_id: String,
        template: &Template,
        shared_buffer: SharedArrayBuffer,
    ) -> Result<()> {
        self.publish_manager
            .publish_event(publish_id, template, shared_buffer)
            .await
    }
}
