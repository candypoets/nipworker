use nipworker_core::{
    channel::{WasmWorkerChannel, WorkerChannel},
    storage::NostrDbStorage,
    worker::cache_worker::CacheWorker,
};
use std::sync::Arc;
use wasm_bindgen::prelude::*;
use web_sys::MessagePort;

use std::sync::Once;

static INIT: Once = Once::new();

const DEFAULT_RELAYS: &[&str] = &[
    "wss://relay.snort.social",
    "wss://relay.damus.io",
    "wss://relay.primal.net",
];
const INDEXER_RELAYS: &[&str] = &[
    "wss://user.kindpag.es",
    "wss://relay.nos.social",
    "wss://purplepag.es",
    "wss://profiles.nostr1.com",
];

fn js_array_to_strings(value: &JsValue) -> Vec<String> {
    if !js_sys::Array::is_array(value) {
        return Vec::new();
    }
    js_sys::Array::from(value)
        .iter()
        .filter_map(|v| v.as_string())
        .filter(|v| !v.trim().is_empty())
        .collect()
}

#[wasm_bindgen]
pub fn init_tracing(level: String) {
    INIT.call_once(|| {
        let max_level = match level.to_lowercase().as_str() {
            "trace" => tracing::Level::TRACE,
            "debug" => tracing::Level::DEBUG,
            "info" => tracing::Level::INFO,
            "warn" => tracing::Level::WARN,
            "error" => tracing::Level::ERROR,
            _ => tracing::Level::INFO,
        };
        let mut builder = tracing_wasm::WASMLayerConfigBuilder::new();
        builder.set_max_level(max_level);
        tracing_wasm::set_as_global_default_with_config(builder.build());
        console_error_panic_hook::set_once();
    });
}

/// Start the cache worker with two MessageChannel ports:
/// - `parser_port`:   bidirectional channel with the parser worker
/// - `connections_port`: send-only channel to the connections worker
#[wasm_bindgen]
pub fn start_worker(
    parser_port: MessagePort,
    connections_port: MessagePort,
    default_relays: JsValue,
    indexer_relays: JsValue,
) {
    let parser_ch = WasmWorkerChannel::new(parser_port);
    let to_parser = parser_ch.clone_sender();
    let from_parser = Box::new(parser_ch);

    let to_connections = WasmWorkerChannel::new(connections_port).clone_sender();
    let default_relays = {
        let relays = js_array_to_strings(&default_relays);
        if relays.is_empty() {
            DEFAULT_RELAYS.iter().map(|s| s.to_string()).collect()
        } else {
            relays
        }
    };
    let indexer_relays = {
        let relays = js_array_to_strings(&indexer_relays);
        if relays.is_empty() {
            INDEXER_RELAYS.iter().map(|s| s.to_string()).collect()
        } else {
            relays
        }
    };

    let storage = Arc::new(NostrDbStorage::new(
        "nipworker".to_string(),
        8 * 1024 * 1024,
        default_relays,
        indexer_relays,
    ));

    let worker = CacheWorker::new(storage);
    worker.run(from_parser, to_parser, to_connections);
}
