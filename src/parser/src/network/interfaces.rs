use crate::{types::parsed_event::ParsedEvent, NostrError};
use shared::types::{network::Request, Event, Filter};

type Result<T> = std::result::Result<T, NostrError>;

// No async_trait or Send + Sync needed for WASM
pub trait EventDatabase {
    async fn query_events_for_requests(
        &self,
        requests: Vec<Request>,
        skip_filtered: bool,
    ) -> Result<(Vec<Request>, Vec<Vec<u8>>)>;

    async fn query_events(&self, filter: Filter) -> Result<Vec<Vec<u8>>>;
    async fn add_event(&self, event: &ParsedEvent) -> Result<()>;
}

pub trait EventParser {
    async fn parse(&self, event: Event) -> Result<ParsedEvent>;
    fn get_relay_hint(&self, event: &Event) -> Vec<String>;
    fn get_relays(&self, kind: u64, pubkey: &str, write: bool) -> Vec<String>;
}

pub trait JavaScriptBridge {
    fn post_message(&self, event_type: &str, subscription_id: &str, data: &str);
}

pub trait SubscriptionOptimizer {
    fn optimize_subscriptions(&self, requests: Vec<Request>) -> Vec<Request>;
}

pub trait CacheProcessor {
    async fn process_local_requests(
        &self,
        requests: Vec<Request>,
    ) -> Result<(Vec<Request>, Vec<Vec<Vec<u8>>>)>;

    // async fn find_event_context(&self, event: &ParsedEvent, max_depth: usize) -> Vec<Vec<u8>>;
}

pub trait EventStagingManager {
    async fn stage_event(&self, event: ParsedEvent);
    async fn start_staging_process(&self);
}
