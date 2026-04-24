use nipworker_core::{
	channel::{WasmWorkerChannel, WorkerChannel},
	worker::crypto_worker::CryptoWorker,
};
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

/// Start the crypto worker with three MessageChannel ports:
/// - `engine_port`:      bidirectional channel with the main thread
/// - `parser_port`:      bidirectional channel with the parser worker
/// - `connections_port`: bidirectional channel with the connections worker
#[wasm_bindgen]
pub fn start_worker(
	engine_port: MessagePort,
	parser_port: MessagePort,
	connections_port: MessagePort,
) {
	let engine_ch = WasmWorkerChannel::new(engine_port);
	let to_main = engine_ch.clone_sender();
	let from_engine = Box::new(engine_ch);

	let parser_ch = WasmWorkerChannel::new(parser_port);
	let to_parser = parser_ch.clone_sender();
	let from_parser = Box::new(parser_ch);

	let conn_ch = WasmWorkerChannel::new(connections_port);
	let to_connections = conn_ch.clone_sender();
	let from_connections = Box::new(conn_ch);

	let worker = CryptoWorker::new();
	worker.run(
		from_engine,
		from_parser,
		from_connections,
		to_main,
		to_parser,
		to_connections,
	);
}
