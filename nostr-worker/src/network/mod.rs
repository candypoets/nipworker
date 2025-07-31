pub mod cache_processor;
pub mod interfaces;
pub mod optimizer;
pub mod publish;
pub mod subscription;

use crate::db::NostrDB;
use crate::parser::Parser;
use crate::relays::ConnectionRegistry;
use crate::types::network::Request;
use crate::types::*;
use anyhow::Result;
use js_sys::SharedArrayBuffer;
use nostr::UnsignedEvent;
use std::sync::Arc;

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

        let subscription_manager =
            subscription::SubscriptionManager::new(database.clone(), parser.clone());

        Self {
            publish_manager,
            subscription_manager,
        }
    }

    pub async fn open_subscription(
        &self,
        subscription_id: String,
        requests: Vec<Request>,
        shared_buffer: SharedArrayBuffer,
        config: Option<SubscriptionConfig>,
    ) -> Result<()> {
        self.subscription_manager
            .open_subscription(subscription_id, requests, shared_buffer, config)
            .await
    }

    pub async fn close_subscription(&self, subscription_id: String) -> Result<()> {
        self.subscription_manager
            .close_subscription(&subscription_id)
            .await
    }

    pub async fn publish_event(&self, publish_id: String, event: &mut UnsignedEvent) -> Result<()> {
        self.publish_manager.publish_event(publish_id, event).await
    }

    pub async fn get_active_subscription_count(&self) -> u32 {
        0
    }
}
