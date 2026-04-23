use nipworker_core::{
	channel::{ChannelPort, WasmWorkerChannel, WorkerChannel},
	crypto_client::CryptoClient,
	parser::Parser,
	worker::parser_worker::ParserWorker,
};
use std::sync::Arc;
use wasm_bindgen::prelude::*;
use web_sys::MessagePort;

#[wasm_bindgen(start)]
pub fn start() {
	tracing_wasm::set_as_global_default();
	console_error_panic_hook::set_once();
}

/// Start the parser worker with four MessageChannel ports:
/// - `engine_port`:      bidirectional channel with the main thread
/// - `connections_port`: receive-only channel from the connections worker
/// - `cache_port`:       bidirectional channel with the cache worker
/// - `crypto_port`:      bidirectional channel with the crypto worker
#[wasm_bindgen]
pub fn start_worker(
	engine_port: MessagePort,
	connections_port: MessagePort,
	cache_port: MessagePort,
	crypto_port: MessagePort,
) {
	let engine_ch = WasmWorkerChannel::new(engine_port);
	let to_main = engine_ch.clone_sender();
	let from_engine = Box::new(engine_ch);

	let from_connections = Box::new(WasmWorkerChannel::new(connections_port));

	let cache_ch = WasmWorkerChannel::new(cache_port);
	let to_cache = Arc::new(ChannelPort::new(cache_ch.clone_sender()));
	let from_cache = Box::new(cache_ch);

	let crypto_client = CryptoClient::new(Box::new(WasmWorkerChannel::new(crypto_port)));
	let parser = Arc::new(Parser::new(Some(Arc::new(crypto_client))));

	let worker = ParserWorker::new(parser, to_cache, to_main);
	worker.run(from_engine, from_connections, from_cache);
}
