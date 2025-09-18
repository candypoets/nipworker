pub mod cache_processor;
pub mod interfaces;
pub mod publish;
pub mod subscription;

use crate::db::NostrDB;
use crate::generated::nostr::fb;
use crate::nostr::Template;
use crate::parser::Parser;
use crate::relays::ConnectionRegistry;
use crate::NostrError;
use js_sys::SharedArrayBuffer;
use std::sync::Arc;

type Result<T> = std::result::Result<T, NostrError>;

pub struct NetworkManager {
    publish_manager: publish::PublishManager,
    subscription_manager: subscription::SubscriptionManager,
}

impl NetworkManager {
    pub fn new(
        database: Arc<NostrDB>,
        connection_registry: Arc<ConnectionRegistry>,
        parser: Arc<Parser>,
    ) -> Self {
        let publish_manager = publish::PublishManager::new(
            database.clone(),
            connection_registry.clone(),
            parser.clone(),
        );

        let subscription_manager = subscription::SubscriptionManager::new(
            database.clone(),
            connection_registry.clone(),
            parser.clone(),
        );

        Self {
            publish_manager,
            subscription_manager,
        }
    }

    pub async fn open_subscription(
        &self,
        subscription_id: String,
        shared_buffer: SharedArrayBuffer,
        requests: &Vec<fb::Request<'_>>,
        config: &fb::SubscriptionConfig<'_>,
    ) -> Result<()> {
        self.subscription_manager
            .open_subscription(subscription_id, shared_buffer, &requests, config)
            .await
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

    pub async fn get_active_subscription_count(&self) -> u32 {
        0
    }
}
