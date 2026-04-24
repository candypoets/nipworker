use crate::traits::{RelayTransport, TransportError, TransportStatus};
use async_trait::async_trait;
use futures::channel::mpsc;
use futures::future::{AbortHandle, Abortable};
use futures::lock::Mutex;
use futures::stream::SplitSink;
use futures::{SinkExt, StreamExt};
use gloo_net::websocket::{futures::WebSocket, Message};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use wasm_bindgen_futures::spawn_local;

struct WsHandle {
	write: Mutex<SplitSink<WebSocket, Message>>,
	abort: AbortHandle,
}

pub struct GlooTransport {
	connections: Rc<RefCell<HashMap<String, Rc<WsHandle>>>>,
	message_callbacks: Rc<RefCell<HashMap<String, Box<dyn Fn(String)>>>>,
	status_callbacks: Rc<RefCell<HashMap<String, Box<dyn Fn(TransportStatus)>>>>,
}

impl GlooTransport {
	pub fn new() -> Self {
		Self {
			connections: Rc::new(RefCell::new(HashMap::new())),
			message_callbacks: Rc::new(RefCell::new(HashMap::new())),
			status_callbacks: Rc::new(RefCell::new(HashMap::new())),
		}
	}
}

#[async_trait(?Send)]
impl RelayTransport for GlooTransport {
	async fn connect(&self, url: &str) -> Result<(), TransportError> {
		// Tear down any existing socket for this URL before replacing it.
		self.disconnect(url);

		let ws = WebSocket::open(url)
			.map_err(|e| TransportError::Other(format!("WebSocket connect failed: {}", e)))?;

		let (write, mut read) = ws.split();

		let url_reader = url.to_string();
		let status_cbs = self.status_callbacks.clone();
		let msg_cbs = self.message_callbacks.clone();
		let connections = self.connections.clone();

		let (abort_handle, reg) = AbortHandle::new_pair();

		// Reader task
		let reader_fut = async move {
			while let Some(msg) = read.next().await {
				match msg {
					Ok(Message::Text(text)) => {
						if let Some(cb) = msg_cbs.borrow().get(&url_reader) {
							cb(text);
						}
					}
					Ok(_) => {}
					Err(_) => break,
				}
			}
			// Stream ended (peer close, abort, or error).
			connections.borrow_mut().remove(&url_reader);
			if let Some(cb) = status_cbs.borrow().get(&url_reader) {
				cb(TransportStatus::Closed {
					url: url_reader.clone(),
				});
			}
		};

		let abortable_reader = Abortable::new(reader_fut, reg);

		spawn_local(async move {
			let _ = abortable_reader.await;
		});

		self.connections.borrow_mut().insert(
			url.to_string(),
			Rc::new(WsHandle {
				write: Mutex::new(write),
				abort: abort_handle,
			}),
		);

		if let Some(cb) = self.status_callbacks.borrow().get(url) {
			cb(TransportStatus::Connected {
				url: url.to_string(),
			});
		}

		Ok(())
	}

	fn disconnect(&self, url: &str) {
		if let Some(handle) = self.connections.borrow_mut().remove(url) {
			handle.abort.abort();
		}
		if let Some(cb) = self.status_callbacks.borrow().get(url) {
			cb(TransportStatus::Closed {
				url: url.to_string(),
			});
		}
	}

	async fn send(&self, url: &str, frame: String) -> Result<(), TransportError> {
		let handle = {
			let conns = self.connections.borrow();
			conns.get(url).cloned()
		};
		if let Some(handle) = handle {
			let mut sink = handle.write.lock().await;
			sink.send(Message::Text(frame))
				.await
				.map_err(|e| TransportError::Other(format!("Send failed: {}", e)))
		} else {
			Err(TransportError::Other(format!("Not connected to {}", url)))
		}
	}

	fn on_message(&self, url: &str, callback: Box<dyn Fn(String)>) {
		self.message_callbacks
			.borrow_mut()
			.insert(url.to_string(), callback);
	}

	fn on_status(&self, url: &str, callback: Box<dyn Fn(TransportStatus)>) {
		self.status_callbacks
			.borrow_mut()
			.insert(url.to_string(), callback);
	}
}
