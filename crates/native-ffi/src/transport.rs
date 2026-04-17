use nipworker_core::traits::{RelayTransport, TransportError, TransportStatus};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use futures::channel::mpsc;
use futures::{SinkExt, StreamExt};

struct WsHandle {
    write: mpsc::UnboundedSender<String>,
    close_tx: tokio::sync::mpsc::UnboundedSender<()>,
}

pub struct NativeTransport {
    connections: Rc<RefCell<HashMap<String, WsHandle>>>,
    message_callbacks: Rc<RefCell<HashMap<String, Box<dyn Fn(String)>>>>,
    status_callbacks: Rc<RefCell<HashMap<String, Box<dyn Fn(TransportStatus)>>>>,
}

impl NativeTransport {
    pub fn new() -> Self {
        Self {
            connections: Rc::new(RefCell::new(HashMap::new())),
            message_callbacks: Rc::new(RefCell::new(HashMap::new())),
            status_callbacks: Rc::new(RefCell::new(HashMap::new())),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl RelayTransport for NativeTransport {
    async fn connect(&self, url: &str) -> Result<(), TransportError> {
        use tokio_tungstenite::connect_async;
        use tokio_tungstenite::tungstenite::Message;

        let (ws_stream, _) = connect_async(url)
            .await
            .map_err(|e| TransportError::Other(format!("WebSocket connect failed: {}", e)))?;

        let (mut write, mut read) = ws_stream.split();
        let (tx, mut rx) = mpsc::unbounded::<String>();
        let (close_tx, mut close_rx) = tokio::sync::mpsc::unbounded_channel::<()>();

        let url_writer = url.to_string();
        let url_reader = url.to_string();
        let status_cbs = self.status_callbacks.clone();
        let msg_cbs = self.message_callbacks.clone();
        let connections = self.connections.clone();

        // Writer task
        let status_cbs_writer = status_cbs.clone();
        tokio::task::spawn_local(async move {
            while let Some(msg) = rx.next().await {
                if write.send(Message::Text(msg)).await.is_err() {
                    break;
                }
            }
            connections.borrow_mut().remove(&url_writer);
            if let Some(cb) = status_cbs_writer.borrow().get(&url_writer) {
                cb(TransportStatus::Closed { url: url_writer.clone() });
            }
        });

        // Reader task
        let status_cbs_reader = status_cbs.clone();
        tokio::task::spawn_local(async move {
            loop {
                tokio::select! {
                    msg = read.next() => {
                        match msg {
                            Some(Ok(Message::Text(text))) => {
                                if let Some(cb) = msg_cbs.borrow().get(&url_reader) {
                                    cb(text);
                                }
                            }
                            Some(Ok(_)) => {}
                            Some(Err(_)) | None => break,
                        }
                    }
                    _ = close_rx.recv() => break,
                }
            }
            if let Some(cb) = status_cbs_reader.borrow().get(&url_reader) {
                cb(TransportStatus::Closed { url: url_reader.clone() });
            }
        });

        self.connections.borrow_mut().insert(url.to_string(), WsHandle { write: tx, close_tx });
        if let Some(cb) = self.status_callbacks.borrow().get(url) {
            cb(TransportStatus::Connected { url: url.to_string() });
        }

        Ok(())
    }

    fn disconnect(&self, url: &str) {
        if let Some(handle) = self.connections.borrow_mut().remove(url) {
            let _ = handle.close_tx.send(());
        }
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
