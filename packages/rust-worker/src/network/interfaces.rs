use crate::types::{network::Request, *};
use anyhow::Result;
use async_trait::async_trait;
use futures::channel::mpsc;
use std::sync::Arc;

#[async_trait(?Send)]
pub trait EventDatabase: Send + Sync {
    async fn query_events_for_requests(
        &self,
        requests: Vec<Request>,
        skip_filtered: bool,
    ) -> Result<(Vec<Request>, Vec<ParsedEvent>)>;

    async fn query_events(&self, filter: nostr::Filter) -> Result<Vec<ParsedEvent>>;
    async fn add_event(&self, event: ParsedEvent) -> Result<()>;
    async fn save_events_to_persistent_storage(&self, events: Vec<ParsedEvent>) -> Result<()>;
}

#[async_trait]
pub trait EventParser: Send + Sync {
    async fn parse(&self, event: nostr::Event) -> Result<ParsedEvent>;
    fn get_relay_hint(&self, event: &nostr::Event) -> Vec<String>;
    fn get_relays(&self, kind: u64, pubkey: &str, write: bool) -> Vec<String>;
}

#[async_trait]
pub trait RelayManager: Send + Sync {
    async fn get_relay(&self, url: &str) -> Result<Arc<dyn RelayConnection>>;
    async fn release_relay(&self, url: &str);
    async fn mark_relay_as_closed(&self, url: &str, error: Option<String>);
}

#[async_trait]
pub trait RelayConnection: Send + Sync {
    async fn subscribe(&self, filter: nostr::Filter) -> Result<mpsc::Receiver<NetworkEvent>>;
    async fn publish(&self, event: &nostr::Event) -> Result<()>;
    async fn close(&self);
    fn url(&self) -> &str;
    fn is_connected(&self) -> bool;
}

pub trait JavaScriptBridge: Send + Sync {
    fn post_message(&self, event_type: &str, subscription_id: &str, data: &str);
}

#[async_trait]
pub trait SubscriptionOptimizer: Send + Sync {
    fn optimize_subscriptions(&self, requests: Vec<Request>) -> Vec<Request>;
}

#[async_trait(?Send)]
pub trait CacheProcessor: Send + Sync {
    async fn process_local_requests(
        &self,
        requests: Vec<Request>,
        max_depth: usize,
    ) -> Result<(Vec<Request>, Vec<Vec<ParsedEvent>>)>;

    async fn find_event_context(&self, event: &ParsedEvent, max_depth: usize) -> Vec<ParsedEvent>;
}

#[async_trait(?Send)]
pub trait NetworkProcessor: Send + Sync {
    async fn process_network_requests(
        &self,
        requests: Vec<Request>,
    ) -> mpsc::Receiver<NetworkEvent>;
}

#[async_trait]
pub trait SubscriptionTrait: Send + Sync {
    fn id(&self) -> &str;
    async fn is_cancelled(&self) -> bool;
    async fn cancel(&self);

    async fn get_sent_events(&self) -> std::collections::HashMap<String, Vec<ParsedEvent>>;
    async fn mark_event_as_sent(&self, event_id: &str, events: Vec<ParsedEvent>);
    async fn has_event_been_sent(&self, event_id: &str) -> bool;

    async fn add_to_fetched_batch(&self, events: Vec<ParsedEvent>);
    async fn get_fetched_batch(&self) -> Vec<Vec<ParsedEvent>>;
    async fn clear_fetched_batch(&self);
    async fn is_in_batching_mode(&self) -> bool;
    async fn set_batching_mode(&self, batching: bool);
}

#[async_trait]
pub trait SubscriptionRegistry: Send + Sync {
    async fn create(&self, id: String) -> Arc<dyn SubscriptionTrait>;
    async fn get(&self, id: &str) -> Option<Arc<dyn SubscriptionTrait>>;
    async fn remove(&self, id: &str);
    async fn count(&self) -> usize;
    async fn list(&self) -> Vec<String>;
    async fn cleanup(&self);
}

#[async_trait]
pub trait EventStagingManager: Send + Sync {
    async fn stage_event(&self, event: ParsedEvent);
    async fn start_staging_process(&self);
}
