use crate::channel::{WorkerChannel, WorkerChannelSender};
use crate::spawn::spawn_worker;
use crate::traits::Storage;
use std::sync::Arc;
use tracing::info;

pub struct CacheWorker {
	_storage: Arc<dyn Storage>,
}
impl CacheWorker {
	pub fn new(storage: Arc<dyn Storage>) -> Self { Self { _storage: storage } }
	pub fn run(self, mut from_parser: Box<dyn WorkerChannel>, _to_parser: Box<dyn WorkerChannelSender>) {
		spawn_worker(async move {
			info!("[CacheWorker] started");
			while let Ok(_bytes) = from_parser.recv().await {
				// TODO: decode CacheRequest, query/persist storage, send CacheResponse back to parser
			}
			info!("[CacheWorker] channel closed, exiting");
		});
	}
}
