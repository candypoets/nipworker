//! Individual Relay Connection Management
//!
//! Revised design goals:
//! - Connect immediately on construction.
//! - Enqueue frames immediately; only start draining once the connection is established.
//! - If not connected while draining, pause reading, attempt reconnection, then retry the frame.
//!   If the frame cannot be sent (send error or reconnection failure), push it back to the queue.
//! - Synthetic notifications are emitted on successful send: REQ => SUBSCRIBED, CLOSE => CLOSED.
//!
//! Notes:
//! - Exactly one active WebSocket per RelayConnection: abort previous reader and close sink before opening a new socket.
//! - Synchronous, non-blocking enqueue via bounded channel (cap: 50). `send_raw` never awaits network.
//! - Incoming messages are written to ring buffer via `out_writer`. Status changes via `status_writer`.

use crate::types::{ConnectionStats, ConnectionStatus, RelayConfig, RelayError};
use crate::utils::{extract_first_three, validate_relay_url};

use futures::channel::mpsc::{self, Receiver, Sender};
use futures::future::{AbortHandle, Abortable};
use futures::lock::Mutex;
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use gloo_net::websocket::{futures::WebSocket, Message};
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::{Arc, RwLock};
use wasm_bindgen_futures::spawn_local;

type OutWriter = Rc<dyn Fn(&str, &str, &str)>; // (url, sub_id, raw_text)
type StatusWriter = Rc<dyn Fn(&str, &str)>; // (status, url)

pub struct RelayConnection {
    url: String,
    config: RelayConfig,

    status: Arc<RwLock<ConnectionStatus>>,
    websocket: Arc<RwLock<Option<WebSocket>>>,
    ws_sink: Arc<Mutex<Option<SplitSink<WebSocket, Message>>>>,

    stats: Arc<RwLock<ConnectionStats>>,
    active_subs: Arc<RwLock<HashSet<String>>>,
    backoff_attempts: Arc<RwLock<u32>>,
    next_retry_at_ms: Arc<RwLock<u64>>,

    // Channel created at construction time so callers can enqueue immediately.
    queue_tx: Arc<RwLock<Option<Sender<String>>>>,
    // Receiver is held until first successful connect, then consumed by the drainer.
    queue_rx: Arc<RwLock<Option<Receiver<String>>>>,

    read_abort: Arc<RwLock<Option<AbortHandle>>>,

    out_writer: OutWriter,
    status_writer: StatusWriter,
}

impl RelayConnection {
    pub fn new(
        url: String,
        config: RelayConfig,
        out_writer: OutWriter,
        status_writer: StatusWriter,
    ) -> Arc<Self> {
        // Create the queue immediately so send_raw can enqueue even before connection.
        let (tx, rx) = mpsc::channel::<String>(50);

        let conn = Arc::new(Self {
            url,
            config,
            status: Arc::new(RwLock::new(ConnectionStatus::Connecting)),
            websocket: Arc::new(RwLock::new(None)),
            ws_sink: Arc::new(Mutex::new(None)),
            stats: Arc::new(RwLock::new(ConnectionStats::default())),
            active_subs: Arc::new(RwLock::new(HashSet::new())),
            backoff_attempts: Arc::new(RwLock::new(0)),
            next_retry_at_ms: Arc::new(RwLock::new(0)),
            queue_tx: Arc::new(RwLock::new(Some(tx))),
            queue_rx: Arc::new(RwLock::new(Some(rx))),
            read_abort: Arc::new(RwLock::new(None)),
            out_writer,
            status_writer,
        });

        // Connect immediately
        (conn.status_writer)("connecting", &conn.url);
        let this = Arc::clone(&conn);
        spawn_local(async move {
            if let Err(e) = this.connect().await {
                tracing::error!(relay = %this.url, error = ?e, "Initial connect failed");
                (this.status_writer)("failed", &this.url);
            }
        });

        conn
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    #[inline]
    fn clear_backoff(&self) {
        *self.backoff_attempts.write().unwrap() = 0;
        *self.next_retry_at_ms.write().unwrap() = 0;
    }

    // Spawn the reader task for this connection. Non-async: it only spawns and returns.
    fn spawn_reader(self: &Arc<Self>, mut ws_stream: SplitStream<WebSocket>) {
        let status = self.status.clone();
        let ws_sink = self.ws_sink.clone();
        let url = self.url.clone();
        let out_writer = self.out_writer.clone();
        let status_writer = self.status_writer.clone();

        let fut = async move {
            while let Some(msg) = ws_stream.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        tracing::info!(relay = %url, "Raw incoming: {}", text);
                        if let Some(parts) = extract_first_three(&text) {
                            if let Some(kind_raw) = parts[0] {
                                let kind = kind_raw.trim_matches('"');
                                let sub_id = parts[1]
                                    .map(|s| s.trim_matches('"').to_string())
                                    .unwrap_or_default();
                                let content = parts[2].unwrap_or("");
                                let raw = match kind {
                                    "EVENT" => format!(r#"["EVENT","{}",{}]"#, sub_id, content),
                                    "EOSE" => format!(r#"["EOSE","{}"]"#, sub_id),
                                    "OK" => format!(r#"["OK","{}",{}]"#, sub_id, content),
                                    "CLOSED" => {
                                        let esc =
                                            content.replace('\\', "\\\\").replace('"', "\\\"");
                                        format!(r#"["CLOSED","{}","{}"]"#, sub_id, esc)
                                    }
                                    "NOTICE" => {
                                        let esc =
                                            content.replace('\\', "\\\\").replace('"', "\\\"");
                                        format!(r#"["NOTICE","{}"]"#, esc)
                                    }
                                    "AUTH" => {
                                        let esc =
                                            content.replace('\\', "\\\\").replace('"', "\\\"");
                                        format!(r#"["AUTH","{}"]"#, esc)
                                    }
                                    other => {
                                        let esc = other.replace('\\', "\\\\").replace('"', "\\\"");
                                        format!(r#"["NOTICE","unknown kind: {}"]"#, esc)
                                    }
                                };
                                if !sub_id.is_empty() {
                                    (out_writer)(&url, &sub_id, &raw);
                                }
                            }
                        }
                    }
                    Ok(Message::Bytes(_)) => {
                        tracing::warn!(relay = %url, "Unexpected binary message");
                    }
                    Err(e) => {
                        tracing::error!(relay = %url, error = %e, "WebSocket error");
                        {
                            let mut st = status.write().unwrap();
                            *st = ConnectionStatus::Failed;
                        }
                        {
                            let mut sink_guard = ws_sink.lock().await;
                            *sink_guard = None;
                        }
                        (status_writer)("failed", &url);
                        break;
                    }
                }
            }
            tracing::debug!(relay = %url, "reader: stream ended/aborted");
        };

        // Make abortable and remember handle
        let (handle, reg) = AbortHandle::new_pair();
        {
            let mut ah = self.read_abort.write().unwrap();
            *ah = Some(handle);
        }
        let task = Abortable::new(fut, reg);
        spawn_local(async move {
            let _ = task.await;
        });
    }

    // Initialize queue drainer once, after a successful connect: take the receiver and spawn drainer.
    fn init_queue_drainer(self: &Arc<Self>) {
        // Only start the drainer if we still have the receiver (i.e., not started yet).
        let maybe_rx = { self.queue_rx.write().unwrap().take() };
        if let Some(rx) = maybe_rx {
            let conn = Arc::clone(self);
            spawn_local(async move {
                conn.queue_drainer(rx).await;
            });
        }
    }

    // Drainer owns Arc<Self> to keep the connection alive while draining
    async fn queue_drainer(self: Arc<Self>, mut rx: Receiver<String>) {
        while let Some(frame) = rx.next().await {
            // Fast path: if connected, try send
            if matches!(*self.status.read().unwrap(), ConnectionStatus::Connected) {
                match Self::send_raw_internal(&self, &frame).await {
                    Ok(()) => {
                        // (self.status_writer)("connected", &self.url);
                        continue;
                    }
                    Err(e) => {
                        tracing::error!(relay = %self.url, error = ?e, "Send failed while connected; will requeue and attempt reconnect");
                        // (self.status_writer)("failed", &self.url);
                        // fallthrough to reconnect
                    }
                }
            }

            // Not connected OR send failed: attempt reconnection now, pausing queue consumption
            let reconnect_result = self.connect().await;
            if reconnect_result.is_ok()
                && matches!(*self.status.read().unwrap(), ConnectionStatus::Connected)
            {
                // Reconnected: try sending the same frame again
                match Self::send_raw_internal(&self, &frame).await {
                    Ok(()) => {
                        // (self.status_writer)("connected", &self.url);
                        continue;
                    }
                    Err(e) => {
                        tracing::error!(relay = %self.url, error = ?e, "Send failed after reconnection; pushing frame back");
                        // (self.status_writer)("failed", &self.url);
                        // let _ = self.requeue_frame(frame.clone()).await;
                        // gloo_timers::future::TimeoutFuture::new(200).await;
                        continue;
                    }
                }
            } else {
                // Could not reconnect: push frame back and wait a bit
                // tracing::warn!(relay = %self.url, "Reconnect attempt failed; pushing frame back");
                // let _ = self.requeue_frame(frame.clone()).await;
                // gloo_timers::future::TimeoutFuture::new(300).await;
                continue;
            }
        }
        tracing::debug!(relay = %self.url, "Queue drainer exiting");
    }

    async fn send_raw_internal(self: &Arc<Self>, text: &str) -> Result<(), RelayError> {
        // Lock sink and send
        let mut sink_guard = self.ws_sink.lock().await;
        let sink = sink_guard.as_mut().ok_or(RelayError::ConnectionClosed)?;

        if let Err(e) = sink.send(Message::Text(text.to_owned())).await {
            tracing::error!(error = %e, "Send error; closing sink");
            {
                let mut st = self.status.write().unwrap();
                *st = ConnectionStatus::Failed;
            }
            let _ = futures::SinkExt::close(sink).await;
            *sink_guard = None;
            // (self.status_writer)("failed", &self.url);
            return Err(RelayError::ConnectionClosed);
        }

        // On successful send, adjust inflight and emit synthetic notifications when appropriate.
        if let Some(parts) = extract_first_three(text) {
            if let Some(kind_raw) = parts[0] {
                let k = kind_raw.trim_matches('"');
                match k {
                    "CLOSE" => {
                        if let Some(sub) = parts[1].map(|s| s.trim_matches('"').to_string()) {
                            // New: use membership instead of a counter
                            {
                                let mut set = self.active_subs.write().unwrap();
                                set.remove(&sub);
                                if set.is_empty() {
                                    // Preserve the existing “auto-close when no subs remain”
                                    drop(sink_guard);
                                    let _ = Self::close(self).await;
                                }
                            }
                            let raw_closed = format!(r#"["OK","{}","CLOSED"]"#, sub);
                            (self.out_writer)(&self.url, &sub, &raw_closed);
                        }
                    }
                    "REQ" => {
                        if let Some(sub_id) = parts[1].map(|s| s.trim_matches('"').to_string()) {
                            self.active_subs.write().unwrap().insert(sub_id.clone());
                            // optional: keep the synthetic notification
                            let raw_subscribed = format!(r#"["OK","{}", SUBSCRIBED]"#, sub_id);
                            (self.out_writer)(&self.url, &sub_id, &raw_subscribed);
                        }
                    }
                    "EVENT" => {
                        // parts[1] is the event JSON object; extract "id":"<hex>"
                        if let Some(event_obj) = parts[1] {
                            // Minimal, allocation-light extraction of the id field
                            let mut event_id: Option<String> = None;
                            if let Some(pos) = event_obj.find("\"id\"") {
                                if let Some(colon) = event_obj[pos..].find(':') {
                                    let rest = &event_obj[pos + colon + 1..];
                                    let rest = rest.trim_start();
                                    if rest.starts_with('"') {
                                        if let Some(endq) = rest[1..].find('"') {
                                            let id = &rest[1..1 + endq];
                                            if !id.is_empty() {
                                                event_id = Some(id.to_string());
                                            }
                                        }
                                    }
                                }
                            }

                            if let Some(id) = event_id {
                                // Synthetic OK to indicate the publish has been sent
                                // This routes by event_id (used as sub_id for publish tracking)
                                let raw_sent = format!(r#"["OK","{}","SENT"]"#, id);
                                (self.out_writer)(&self.url, &id, &raw_sent);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        Ok(())
    }

    // Public API: enqueue a frame (sync, non-blocking).
    // No readiness logic here; the drainer enforces connection state and reconnection.
    pub fn send_raw(self: &Arc<Self>, text: &str) -> Result<(), RelayError> {
        if let Some(tx) = self.queue_tx.read().unwrap().as_ref() {
            tx.clone().try_send(text.to_owned()).map_err(|e| {
                if e.is_full() {
                    RelayError::QueueFull
                } else {
                    RelayError::ConnectionClosed
                }
            })
        } else {
            Err(RelayError::ConnectionClosed)
        }
    }

    async fn connect(self: &Arc<Self>) -> Result<(), RelayError> {
        // If Closed explicitly, do not reconnect
        if matches!(*self.status.read().unwrap(), ConnectionStatus::Closed) {
            return Err(RelayError::ConnectionClosed);
        }

        // Abort previous reader and close sink before opening new socket
        {
            if let Some(handle) = self.read_abort.write().unwrap().take() {
                handle.abort();
            }
            let mut sink_guard = self.ws_sink.lock().await;
            if let Some(sink) = sink_guard.as_mut() {
                let _ = SinkExt::close(sink).await;
            }
            *sink_guard = None;
        }

        validate_relay_url(&self.url)?;

        // Open socket
        let ws = WebSocket::open(&self.url).map_err(|e| {
            let mut st = self.status.write().unwrap();
            *st = ConnectionStatus::Failed;
            (self.status_writer)("failed", &self.url);
            RelayError::WebSocketError(e.to_string())
        })?;

        {
            let mut ws_guard = self.websocket.write().unwrap();
            *ws_guard = Some(ws);
        }

        // Take ownership and split
        let ws_for_split = {
            let mut ws_guard = self.websocket.write().unwrap();
            ws_guard.take()
        };
        let Some(ws_for_split) = ws_for_split else {
            let mut st = self.status.write().unwrap();
            *st = ConnectionStatus::Failed;
            (self.status_writer)("failed", &self.url);
            return Err(RelayError::ConnectionError("No WebSocket".into()));
        };

        let (ws_sink, ws_stream) = ws_for_split.split();
        {
            let mut sink_guard = self.ws_sink.lock().await;
            *sink_guard = Some(ws_sink);
        }

        // Reader (sync call)
        self.spawn_reader(ws_stream);

        // Mark connected
        {
            let mut st = self.status.write().unwrap();
            *st = ConnectionStatus::Connected;
        }
        {
            let mut s = self.stats.write().unwrap();
            s.connected_at = Some(js_sys::Date::now() as u64);
        }
        (self.status_writer)("connected", &self.url);
        self.clear_backoff();

        // Start the queue drainer on first successful connection
        self.init_queue_drainer();

        Ok(())
    }

    pub async fn close_sub(self: &Arc<Self>, sub_id: &str) -> bool {
        // Fast membership check
        let present = {
            let mut set = self.active_subs.write().unwrap();
            if set.contains(sub_id) {
                set.remove(sub_id);
                true
            } else {
                false
            }
        };
        if !present {
            return false;
        }

        // Send CLOSE only if currently connected; do not attempt reconnect.
        if matches!(*self.status.read().unwrap(), ConnectionStatus::Connected) {
            let mut sink_guard = self.ws_sink.lock().await;
            if let Some(sink) = sink_guard.as_mut() {
                // Best-effort; if it fails, we don’t reconnect here.
                let frame = format!(r#"["CLOSE","{}"]"#, sub_id);
                let _ = sink.send(Message::Text(frame)).await;
            }
        }

        true
    }

    pub async fn close(self: &Arc<Self>) -> Result<(), RelayError> {
        // Abort reader
        {
            if let Some(handle) = self.read_abort.write().unwrap().take() {
                handle.abort();
            }
        }

        // Close sink
        {
            let mut sink_guard = self.ws_sink.lock().await;
            if let Some(sink) = sink_guard.as_mut() {
                let _ = SinkExt::close(sink).await;
            }
            *sink_guard = None;
        }

        // Close raw websocket handle if any (defensive)
        {
            let mut ws_guard = self.websocket.write().unwrap();
            if let Some(ws) = ws_guard.take() {
                let _ = ws.close(None, None);
            }
        }

        {
            let mut st = self.status.write().unwrap();
            *st = ConnectionStatus::Closed;
        }
        (self.status_writer)("close", &self.url);

        Ok(())
    }
}

/*
// Keep Drop disabled while stabilizing (explicit close() and final Arc drop will free resources).
impl Drop for RelayConnection {
    fn drop(&mut self) {
        tracing::info!(relay = %self.url, "Dropping RelayConnection");
        // Any best-effort async cleanup can be reintroduced later if needed.
    }
}
*/
