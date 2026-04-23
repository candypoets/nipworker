use nipworker_core::{
	channel::{WasmWorkerChannel, WorkerChannel},
	storage::NostrDbStorage,
	worker::cache_worker::CacheWorker,
};
use std::sync::Arc;
use wasm_bindgen::prelude::*;
use web_sys::MessagePort;

#[wasm_bindgen(start)]
pub fn start() {
	tracing_wasm::set_as_global_default();
	console_error_panic_hook::set_once();
}

/// Start the cache worker with two MessageChannel ports:
/// - `parser_port`:   bidirectional channel with the parser worker
/// - `connections_port`: send-only channel to the connections worker
#[wasm_bindgen]
pub fn start_worker(parser_port: MessagePort, connections_port: MessagePort) {
	let parser_ch = WasmWorkerChannel::new(parser_port);
	let to_parser = parser_ch.clone_sender();
	let from_parser = Box::new(parser_ch);

	let to_connections = WasmWorkerChannel::new(connections_port).clone_sender();

	let storage = Arc::new(NostrDbStorage::new(
		"nipworker".to_string(),
		8 * 1024 * 1024,
		vec![],
		vec![],
	));

	let worker = CacheWorker::new(storage);
	worker.run(from_parser, to_parser, to_connections);
}
