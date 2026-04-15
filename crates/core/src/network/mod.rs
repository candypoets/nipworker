pub mod interfaces;
pub mod publish;
pub mod subscription;

use crate::crypto_client::CryptoClient;
use crate::parser::Parser;
use crate::nostr_error::NostrError;
use crate::types::nostr::Template;
use crate::types::Event;
use std::sync::Arc;

type Result<T> = std::result::Result<T, NostrError>;

pub struct NetworkManager;

impl NetworkManager {
    pub fn new(
        _parser: Arc<Parser>,
        _to_cache: (),
        _from_connections: (),
        _from_cache: (),
        _crypto_client: Arc<CryptoClient>,
        _to_main: (),
    ) -> Self {
        Self
    }

    pub async fn open_subscription(
        &self,
        _subscription_id: String,
        _requests: Vec<crate::types::network::Request>,
        _filters: Vec<crate::types::Filter>,
        _pipeline_types: Vec<String>,
        _relay_hints: Vec<String>,
        _source: String,
        _publish_id: Option<String>,
        _shard: Option<usize>,
    ) -> Result<()> {
        Ok(())
    }

    pub async fn close_subscription(&self, _subscription_id: String) -> Result<()> {
        Ok(())
    }

    pub async fn publish_event(
        &self,
        _publish_id: String,
        _template: &Template,
    ) -> Result<Event> {
        Err(NostrError::Other("publish_event stub".to_string()))
    }
}
