use crate::channel::{WorkerChannel, WorkerChannelSender};
use crate::spawn::spawn_worker;
use crate::traits::RelayTransport;
use std::sync::Arc;
use tracing::info;

pub struct ConnectionsWorker {
	transport: Arc<dyn RelayTransport>,
}
impl ConnectionsWorker {
	pub fn new(transport: Arc<dyn RelayTransport>) -> Self { Self { transport } }
	pub fn run(self, mut from_parser: Box<dyn WorkerChannel>, _to_parser: Box<dyn WorkerChannelSender>) {
		spawn_worker(async move {
			info!("[ConnectionsWorker] started");
			while let Ok(_bytes) = from_parser.recv().await {
				// TODO: decode WorkerMessage, connect/send via RelayTransport, forward incoming relay frames back to parser
			}
			info!("[ConnectionsWorker] channel closed, exiting");
		});
	}
}
