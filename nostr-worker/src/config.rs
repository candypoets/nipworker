use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NostrConfig {
    pub network: NetworkConfig,
    pub database: DatabaseConfig,
    pub relays: RelayConfig,
    pub subscription: SubscriptionConfig,
    pub parser: ParserConfig,
    pub signer: SignerConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub connection_timeout: u64, // milliseconds
    pub subscription_timeout: u64, // milliseconds
    pub max_concurrent_connections: usize,
    pub retry_attempts: u32,
    pub retry_delay: u64, // milliseconds
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    pub staging_interval: u64, // milliseconds
    pub max_events_in_memory: usize,
    pub enable_persistent_storage: bool,
    pub cleanup_interval: u64, // milliseconds
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayConfig {
    pub default_relays: Vec<String>,
    pub max_pool_size: usize,
    pub connection_keep_alive: u64, // milliseconds
    pub auto_reconnect: bool,
    pub reconnect_delay: u64, // milliseconds
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscriptionConfig {
    pub staging_interval: u64,
    pub max_recursion_depth: usize,
    pub enable_caching: bool,
    pub event_buffer_size: usize,
    pub batch_size: usize,
    pub optimization_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParserConfig {
    pub enable_content_parsing: bool,
    pub max_content_length: usize,
    pub parse_mentions: bool,
    pub parse_hashtags: bool,
    pub parse_urls: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignerConfig {
    pub auto_sign_events: bool,
    pub require_authentication: bool,
    pub key_derivation_iterations: u32,
}

impl Default for NostrConfig {
    fn default() -> Self {
        Self {
            network: NetworkConfig::default(),
            database: DatabaseConfig::default(),
            relays: RelayConfig::default(),
            subscription: SubscriptionConfig::default(),
            parser: ParserConfig::default(),
            signer: SignerConfig::default(),
        }
    }
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            connection_timeout: 10000,
            subscription_timeout: 30000,
            max_concurrent_connections: 20,
            retry_attempts: 3,
            retry_delay: 1000,
        }
    }
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            staging_interval: 5000,
            max_events_in_memory: 10000,
            enable_persistent_storage: true,
            cleanup_interval: 60000,
        }
    }
}

impl Default for RelayConfig {
    fn default() -> Self {
        Self {
            default_relays: vec![
                "wss://relay.damus.io".to_string(),
                "wss://nos.lol".to_string(),
                "wss://relay.primal.net".to_string(),
                "wss://relay.nostr.band".to_string(),
            ],
            max_pool_size: 50,
            connection_keep_alive: 300000, // 5 minutes
            auto_reconnect: true,
            reconnect_delay: 5000,
        }
    }
}

impl Default for SubscriptionConfig {
    fn default() -> Self {
        Self {
            staging_interval: 5000,
            max_recursion_depth: 3,
            enable_caching: true,
            event_buffer_size: 1000,
            batch_size: 100,
            optimization_enabled: true,
        }
    }
}

impl Default for ParserConfig {
    fn default() -> Self {
        Self {
            enable_content_parsing: true,
            max_content_length: 65536, // 64KB
            parse_mentions: true,
            parse_hashtags: true,
            parse_urls: true,
        }
    }
}

impl Default for SignerConfig {
    fn default() -> Self {
        Self {
            auto_sign_events: false,
            require_authentication: true,
            key_derivation_iterations: 100000,
        }
    }
}

impl NostrConfig {
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.network.connection_timeout == 0 {
            return Err("Connection timeout must be greater than 0".to_string());
        }

        if self.network.max_concurrent_connections == 0 {
            return Err("Max concurrent connections must be greater than 0".to_string());
        }

        if self.relays.default_relays.is_empty() {
            return Err("At least one default relay must be configured".to_string());
        }

        if self.subscription.max_recursion_depth > 10 {
            return Err("Max recursion depth should not exceed 10".to_string());
        }

        if self.parser.max_content_length == 0 {
            return Err("Max content length must be greater than 0".to_string());
        }

        Ok(())
    }

    pub fn with_custom_relays(mut self, relays: Vec<String>) -> Self {
        self.relays.default_relays = relays;
        self
    }

    pub fn with_cache_disabled(mut self) -> Self {
        self.subscription.enable_caching = false;
        self
    }

    pub fn with_optimization_disabled(mut self) -> Self {
        self.subscription.optimization_enabled = false;
        self
    }
}