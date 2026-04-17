use crate::channel::{WorkerChannel, WorkerChannelSender};
use crate::spawn::spawn_worker;
use crate::traits::Signer;
use std::sync::Arc;
use tracing::info;

pub struct CryptoWorker {
	signer: Arc<dyn Signer>,
}
impl CryptoWorker {
	pub fn new(signer: Arc<dyn Signer>) -> Self { Self { signer } }
	pub fn run(
		self,
		mut from_engine: Box<dyn WorkerChannel>,
		mut _from_parser: Box<dyn WorkerChannel>,
		_to_main: Box<dyn WorkerChannelSender>,
		_to_parser: Box<dyn WorkerChannelSender>,
	) {
		spawn_worker(async move {
			info!("[CryptoWorker] started");
			loop {
				let _maybe_bytes = from_engine.recv().await;
				if _maybe_bytes.is_err() { break; }
				// TODO: decode request, perform crypto operation, send response
			}
			info!("[CryptoWorker] channels closed, exiting");
		});
	}
}
