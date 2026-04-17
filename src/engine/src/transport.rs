use async_trait::async_trait;
use futures::channel::mpsc;
use futures::{SinkExt, StreamExt};
use nipworker_core::traits::{RelayTransport, TransportError, TransportStatus};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use wasm_bindgen_futures::spawn_local;

struct WsHandle {
    write: mpsc::UnboundedSender<String>,
}

pub struct WebSocketTransport {
    connections: Rc<RefCell<HashMap<String, WsHandle>>>,
    message_callbacks: Rc<RefCell<HashMap<String, Box<dyn Fn(String)>>>>,
    status_callbacks: Rc<RefCell<HashMap<String, Box<dyn Fn(TransportStatus)>>>>,
}

impl WebSocketTransport {
    pub fn new() -> Self {
        Self {
            connections: Rc::new(RefCell::new(HashMap::new())),
            message_callbacks: Rc::new(RefCell::new(HashMap::new())),
            status_callbacks: Rc::new(RefCell::new(HashMap::new())),
        }
    }
}

#[async_trait(?Send)]
impl RelayTransport for WebSocketTransport {
    async fn connect(&self, url: &str) -> Result<(), TransportError> {
        use gloo_net::websocket::futures::WebSocket;
        use gloo_net::websocket::Message;

        let ws = WebSocket::open(url)
            .map_err(|e| TransportError::Other(format!("WebSocket open failed: {:?}", e)))?;

        let (mut write, mut read) = ws.split();
        let (tx, mut rx) = mpsc::unbounded::<String>();

        let url_writer = url.to_string();
        let msg_cbs_reader = self.message_callbacks.clone();
        let status_cbs_reader = self.status_callbacks.clone();
        let connections_reader = self.connections.clone();

        spawn_local(async move {
            while let Some(msg) = rx.next().await {
                if write.send(Message::Text(msg)).await.is_err() {
                    break;
                }
            }
            connections_reader.borrow_mut().remove(&url_writer);
            if let Some(cb) = status_cbs_reader.borrow().get(&url_writer) {
                cb(TransportStatus::Closed { url: url_writer.clone() });
            }
        });

        let url_reader2 = url.to_string();
        spawn_local(async move {
            while let Some(Ok(msg)) = read.next().await {
                if let Message::Text(text) = msg {
                    if let Some(cb) = msg_cbs_reader.borrow().get(&url_reader2) {
                        cb(text);
                    }
                }
            }
        });

        self.connections.borrow_mut().insert(url.to_string(), WsHandle { write: tx });
        if let Some(cb) = self.status_callbacks.borrow().get(url) {
            cb(TransportStatus::Connected { url: url.to_string() });
        }

        Ok(())
    }

    fn disconnect(&self, url: &str) {
        self.connections.borrow_mut().remove(url);
        if let Some(cb) = self.status_callbacks.borrow().get(url) {
            cb(TransportStatus::Closed { url: url.to_string() });
        }
    }

    fn send(&self, url: &str, frame: String) -> Result<(), TransportError> {
        if let Some(handle) = self.connections.borrow().get(url) {
            handle.write.unbounded_send(frame)
                .map_err(|e| TransportError::Other(format!("Send failed: {}", e)))
        } else {
            Err(TransportError::Other(format!("Not connected to {}", url)))
        }
    }

    fn on_message(&self, url: &str, callback: Box<dyn Fn(String)>) {
        self.message_callbacks.borrow_mut().insert(url.to_string(), callback);
    }

    fn on_status(&self, url: &str, callback: Box<dyn Fn(TransportStatus)>) {
        self.status_callbacks.borrow_mut().insert(url.to_string(), callback);
    }
}
