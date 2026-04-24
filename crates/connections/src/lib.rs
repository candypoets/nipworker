use nipworker_core::{
	channel::{WasmWorkerChannel, WorkerChannel},
	transport::gloo::GlooTransport,
	worker::connections_worker::ConnectionsWorker,
};
use std::sync::Arc;
use wasm_bindgen::prelude::*;
use web_sys::MessagePort;

use std::sync::Once;

static INIT: Once = Once::new();

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

/// Start the connections worker with three MessageChannel ports:
/// - `parser_port`:  bidirectional channel with the parser worker
/// - `cache_port`:   receive-only channel from the cache worker
/// - `crypto_port`:  bidirectional channel with the crypto worker
#[wasm_bindgen]
pub fn start_worker(
	parser_port: MessagePort,
	cache_port: MessagePort,
	crypto_port: MessagePort,
) {
	let parser_ch = WasmWorkerChannel::new(parser_port);
	let to_parser = parser_ch.clone_sender();
	let from_parser = Box::new(parser_ch);

	let from_cache = Box::new(WasmWorkerChannel::new(cache_port));

	let crypto_ch = WasmWorkerChannel::new(crypto_port);
	let to_crypto = crypto_ch.clone_sender();
	let from_crypto = Box::new(crypto_ch);

	let transport = Arc::new(GlooTransport::new());
	let worker = ConnectionsWorker::new(transport);
	worker.run(from_parser, to_parser, from_cache, from_crypto, to_crypto);
}
