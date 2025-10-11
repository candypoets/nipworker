use wasm_bindgen::prelude::*;

#[wasm_bindgen]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStatus {
    Idle = 0,
    Connecting = 1,
    Ready = 2,
    Closing = 3,
    Closed = 4,
    Error = 5,
}

#[wasm_bindgen]
#[derive(Clone)]
pub struct RelayConfig {
    pub connect_timeout_ms: u32,
    pub write_timeout_ms: u32,
    pub retry: RetryConfig,
    pub idle_timeout_ms: u32,
    pub max_reconnect_attempts: u32,
}

#[wasm_bindgen]
impl RelayConfig {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            connect_timeout_ms: 5000,
            write_timeout_ms: 10000,
            retry: RetryConfig::new(),
            idle_timeout_ms: 300000,
            max_reconnect_attempts: 2,
        }
    }
}

#[wasm_bindgen]
#[derive(Clone, Copy)]
pub struct RetryConfig {
    pub base_ms: u32,
    pub max_ms: u32,
    pub multiplier: f64,
    pub jitter: f64,
}

#[wasm_bindgen]
impl RetryConfig {
    fn new() -> Self {
        Self {
            base_ms: 300,
            max_ms: 10000,
            multiplier: 1.6,
            jitter: 0.1,
        }
    }
}

#[wasm_bindgen]
#[derive(Clone)]
pub struct RelayStats {
    pub sent: u32,
    pub received: u32,
    pub reconnects: u32,
    pub last_activity: f64,
    pub dropped: u32,
}

#[wasm_bindgen]
pub struct InboundEnvelope {
    pub relays: Vec<String>,
    pub frames: Vec<String>,
}

// MessageHandler as Closure
pub type MessageHandler = Closure<dyn FnMut(String, Option<String>, String)>;

#[wasm_bindgen]
pub enum MsgKind {
    Unknown = 0,
    Event = 1,
    Eose = 2,
    Ok = 3,
    Closed = 4,
    Notice = 5,
    Auth = 6,
}
