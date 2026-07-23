use crate::channel::{MessageSender, WorkerChannel};
use crate::generated::nostr::fb;
use crate::spawn::spawn_worker;
use crate::traits::RelayTransport;
use crate::transport::connection::RelayConnection;
use crate::transport::fb_utils::{build_worker_message, serialize_connection_status};
use crate::transport::frame_scan::scan_relay_frame;
use crate::transport::sub_dedup::{scanned_event_id, SubDedup};
use crate::worker::batch_buffer::{encode_raw_conn_batch, BatchBufferManager};
use futures::StreamExt;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::{Arc, RwLock};
use tracing::{info, warn};

/// How often the connections worker checks connections→parser batch buffers
/// for timeout flushes (same discipline as the parser→main sweeper).
const CONN_BATCH_SWEEP_MS: u64 = 4;

#[derive(serde::Deserialize)]
struct Envelope {
    relays: Vec<String>,
    frames: Vec<String>,
}

fn relay_safe_sub_id(input: &str) -> String {
    if input.len() < 64 {
        return input.to_string();
    }

    let mut hash: i32 = 0;
    for unit in input.encode_utf16() {
        hash = hash
            .wrapping_shl(5)
            .wrapping_sub(hash)
            .wrapping_add(unit as i32);
    }
    let mut value = hash.unsigned_abs();
    if value == 0 {
        return "0".to_string();
    }
    let mut chars = Vec::new();
    while value > 0 {
        let digit = (value % 36) as u8;
        let ch = if digit < 10 {
            (b'0' + digit) as char
        } else {
            (b'a' + digit - 10) as char
        };
        chars.push(ch);
        value /= 36;
    }
    chars.iter().rev().take(63).collect()
}

fn encode_relay_frame(
    frame: &str,
    full_to_relay: &Rc<RefCell<HashMap<String, String>>>,
    relay_to_full: &Rc<RefCell<HashMap<String, String>>>,
) -> String {
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(frame) else {
        return frame.to_string();
    };
    let Some(arr) = value.as_array_mut() else {
        return frame.to_string();
    };
    let Some(kind) = arr.first().and_then(|v| v.as_str()) else {
        return frame.to_string();
    };
    if kind != "REQ" && kind != "CLOSE" {
        return frame.to_string();
    }
    let Some(full_sub_id) = arr.get(1).and_then(|v| v.as_str()).map(str::to_string) else {
        return frame.to_string();
    };

    let relay_sub_id = relay_safe_sub_id(&full_sub_id);
    if relay_sub_id != full_sub_id {
        full_to_relay
            .borrow_mut()
            .insert(full_sub_id.clone(), relay_sub_id.clone());
        relay_to_full
            .borrow_mut()
            .insert(relay_sub_id.clone(), full_sub_id);
        arr[1] = serde_json::Value::String(relay_sub_id);
        serde_json::to_string(&value).unwrap_or_else(|_| frame.to_string())
    } else {
        frame.to_string()
    }
}

fn decode_relay_sub_id(
    sub_id: &str,
    relay_to_full: &Rc<RefCell<HashMap<String, String>>>,
) -> String {
    relay_to_full
        .borrow()
        .get(sub_id)
        .cloned()
        .unwrap_or_else(|| sub_id.to_string())
}

fn relay_frame_state(frame: &str) -> Option<(String, String)> {
    let value = serde_json::from_str::<serde_json::Value>(frame).ok()?;
    let arr = value.as_array()?;
    let kind = arr.first()?.as_str()?.to_string();
    if kind != "REQ" && kind != "CLOSE" {
        return None;
    }
    let sub_id = arr.get(1)?.as_str()?.to_string();
    Some((kind, sub_id))
}

fn send_envelope(
    bytes: &[u8],
    source: &str,
    get_conn: &dyn Fn(&str) -> Arc<RelayConnection>,
    full_to_relay: &Rc<RefCell<HashMap<String, String>>>,
    relay_to_full: &Rc<RefCell<HashMap<String, String>>>,
    sub_relays: &Rc<RefCell<HashMap<String, HashSet<String>>>>,
    sub_dedup: &Rc<RefCell<HashMap<String, SubDedup>>>,
) {
    let env: Envelope = match serde_json::from_slice(bytes) {
        Ok(e) => e,
        Err(_) => {
            warn!(
                "[ConnectionsWorker] Failed to parse envelope from {}",
                source
            );
            return;
        }
    };
    for relay in &env.relays {
        if relay.is_empty() {
            continue;
        }
        let conn = get_conn(relay);
        for frame in &env.frames {
            if let Some((kind, sub_id)) = relay_frame_state(frame) {
                if kind == "REQ" {
                    sub_relays
                        .borrow_mut()
                        .entry(sub_id)
                        .or_default()
                        .insert(relay.clone());
                }
            }
            let relay_frame = encode_relay_frame(frame, full_to_relay, relay_to_full);
            if let Err(e) = conn.send_raw(&relay_frame) {
                warn!(
                    "[ConnectionsWorker] send_raw failed for {} from {}: {:?}",
                    relay, source, e
                );
            }
            if let Some((kind, sub_id)) = relay_frame_state(frame) {
                if kind == "CLOSE" {
                    let should_remove =
                        if let Some(relays) = sub_relays.borrow_mut().get_mut(&sub_id) {
                            relays.remove(relay);
                            relays.is_empty()
                        } else {
                            false
                        };
                    if should_remove {
                        sub_relays.borrow_mut().remove(&sub_id);
                        // Free the cross-relay dedup state for this subscription.
                        sub_dedup.borrow_mut().remove(&sub_id);
                    }
                }
            }
        }
    }
}

pub struct ConnectionsWorker {
    transport: Arc<dyn RelayTransport>,
    connections: Arc<RwLock<HashMap<String, Arc<RelayConnection>>>>,
}

pub struct ConnectionsHandle {
    connections: Arc<RwLock<HashMap<String, Arc<RelayConnection>>>>,
}

impl ConnectionsHandle {
    pub fn wake_all(&self) {
        let connections: Vec<Arc<RelayConnection>> =
            self.connections.read().unwrap().values().cloned().collect();

        info!(
            count = connections.len(),
            "[ConnectionsWorker] waking all relay connections"
        );

        for conn in connections {
            conn.wake();
        }
    }
}

impl ConnectionsWorker {
    pub fn new(transport: Arc<dyn RelayTransport>) -> Self {
        Self {
            transport,
            connections: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn run(
        self,
        mut from_parser: Box<dyn WorkerChannel>,
        to_parser: Box<dyn MessageSender>,
        mut from_cache: Box<dyn WorkerChannel>,
        mut from_crypto: Box<dyn WorkerChannel>,
        to_crypto: Box<dyn MessageSender>,
    ) -> ConnectionsHandle {
        let handle = ConnectionsHandle {
            connections: self.connections.clone(),
        };
        let full_to_relay_sub_ids = Rc::new(RefCell::new(HashMap::<String, String>::new()));
        let relay_to_full_sub_ids = Rc::new(RefCell::new(HashMap::<String, String>::new()));
        let sub_relays = Rc::new(RefCell::new(HashMap::<String, HashSet<String>>::new()));
        // Cross-relay EVENT dedup: one bounded id ring per (full) subscription id.
        // Entries are created lazily on first EVENT and freed on CLOSE.
        let sub_dedup = Rc::new(RefCell::new(HashMap::<String, SubDedup>::new()));

        // Bridge multiple callback clones into the single MessageSender
        let (parser_tx, mut parser_rx) = futures::channel::mpsc::unbounded::<Vec<u8>>();
        spawn_worker(async move {
            while let Some(bytes) = parser_rx.next().await {
                if let Err(e) = to_parser.send(&bytes) {
                    warn!("[ConnectionsWorker] failed to forward to parser: {}", e);
                    break;
                }
            }
        });

        // Per-subscription batch buffers for the connections→parser channel:
        // scanned EVENT frames are concatenated into one channel message
        // instead of one postMessage per frame. Control frames bypass the
        // buffers entirely (see out_writer below).
        let parser_batches = Rc::new(RefCell::new(BatchBufferManager::new()));

        // Batch timeout sweeper: flushes connections→parser batch buffers
        // whose oldest frame exceeded the timeout so events never sit
        // buffered forever.
        {
            let sweep_batches = parser_batches.clone();
            let sweep_tx = parser_tx.clone();
            spawn_worker(async move {
                loop {
                    crate::platform::sleep(CONN_BATCH_SWEEP_MS).await;
                    let payloads = sweep_batches.borrow_mut().drain_timed_out();
                    for payload in payloads {
                        if sweep_tx.unbounded_send(encode_raw_conn_batch(&payload)).is_err() {
                            return;
                        }
                    }
                }
            });
        }

        let to_crypto_rc = std::rc::Rc::new(to_crypto);
        let get_or_create_connection = {
            let transport = self.transport.clone();
            let connections = self.connections.clone();
            let parser_tx = parser_tx.clone();
            let to_crypto_rc = to_crypto_rc.clone();
            let relay_to_full_sub_ids = relay_to_full_sub_ids.clone();
            let sub_dedup = sub_dedup.clone();
            let parser_batches = parser_batches.clone();
            move |url: &str| {
                {
                    let map = connections.read().unwrap();
                    if let Some(conn) = map.get(url) {
                        return conn.clone();
                    }
                }

                let url_string = url.to_string();
                let tx_msg = parser_tx.clone();
                let tx_status = parser_tx.clone();
                let transport = transport.clone();
                let to_crypto_messages = to_crypto_rc.clone();
                let relay_to_full_sub_ids = relay_to_full_sub_ids.clone();
                let sub_dedup_writer = sub_dedup.clone();
                let parser_batches = parser_batches.clone();

                let out_writer: Rc<dyn Fn(&str, &str, &str)> =
                    Rc::new(move |url: &str, sub_id: &str, msg: &str| {
                        let full_sub_id = decode_relay_sub_id(sub_id, &relay_to_full_sub_ids);
                        if full_sub_id.starts_with("n46:") {
                            let mut fbb = flatbuffers::FlatBufferBuilder::new();
                            let wm = build_worker_message(&mut fbb, &full_sub_id, url, msg);
                            fbb.finish(wm, None);
                            // MessageSender accepts &[u8]: send the finished buffer without copying.
                            let _ = to_crypto_messages.send(fbb.finished_data());
                            return;
                        }

                        // Route for this frame, decided by a single zero-copy scan.
                        enum Route<'a> {
                            /// Non-EVENT frame or malformed EVENT: WorkerMessage path.
                            Control,
                            /// Well-formed EVENT: batch the raw event-object JSON slice.
                            Raw(&'a str),
                        }

                        let route = match scan_relay_frame(msg) {
                            Some(scan) if scan.kind == "EVENT" => {
                                // Cross-relay dedup: an EVENT frame reaches the parser
                                // only the first time its (subId, event id) pair is
                                // seen. Non-EVENT frames and unparseable payloads pass
                                // through untouched (parser dedup stays as safety net).
                                if let Some(id) = scanned_event_id(&scan) {
                                    let mut dedup = sub_dedup_writer.borrow_mut();
                                    if !dedup.contains_key(&full_sub_id) {
                                        dedup.insert(full_sub_id.clone(), SubDedup::new());
                                    }
                                    let entry = dedup.get_mut(&full_sub_id).unwrap();
                                    if !entry.mark(id) {
                                        return;
                                    }
                                }
                                match scan.args[1] {
                                    Some(v) if !v.is_string => Route::Raw(v.raw),
                                    _ => Route::Control,
                                }
                            }
                            _ => Route::Control,
                        };

                        match route {
                            Route::Raw(event_json) => {
                                // Compact envelope: the raw event-object slice goes
                                // straight into this subscription's batch buffer —
                                // no FlatBuffer build on the hot path. Flushed by
                                // the size threshold (here), the sweeper timer, or
                                // the next control frame for the sub.
                                let flushed = parser_batches
                                    .borrow_mut()
                                    .add_message(&full_sub_id, event_json.as_bytes());
                                if let Some(payload) = flushed {
                                    let _ =
                                        tx_msg.unbounded_send(encode_raw_conn_batch(&payload));
                                }
                            }
                            Route::Control => {
                                // Control frames (EOSE/CLOSED/OK/AUTH/NOTICE) and
                                // malformed EVENTs must not sit in a buffer: flush
                                // this sub's pending events first so ordering is
                                // preserved, then forward immediately as a bare
                                // single WorkerMessage.
                                let mut fbb = flatbuffers::FlatBufferBuilder::new();
                                let wm = build_worker_message(&mut fbb, &full_sub_id, url, msg);
                                fbb.finish(wm, None);
                                let flushed = parser_batches.borrow_mut().flush_sub(&full_sub_id);
                                if let Some(payload) = flushed {
                                    let _ =
                                        tx_msg.unbounded_send(encode_raw_conn_batch(&payload));
                                }
                                // The mpsc bridge to the parser loop requires an owned Vec.
                                let _ = tx_msg.unbounded_send(fbb.finished_data().to_vec());
                            }
                        }
                    });

                let status_writer: Rc<dyn Fn(&str, &str)> =
                    Rc::new(move |status: &str, url: &str| {
                        let bytes = serialize_connection_status(url, status, "");
                        let _ = tx_status.unbounded_send(bytes);
                    });

                let to_crypto_cb: Rc<RefCell<dyn Fn(&[u8])>> = Rc::new(RefCell::new({
                    let sender = to_crypto_rc.clone();
                    move |bytes: &[u8]| {
                        let _ = sender.send(bytes);
                    }
                }));

                let conn = RelayConnection::new(
                    url_string,
                    transport,
                    out_writer,
                    status_writer,
                    to_crypto_cb,
                );

                {
                    let mut map = connections.write().unwrap();
                    map.insert(url.to_string(), conn.clone());
                }

                conn
            }
        };

        // Loop for messages from parser (e.g. CLOSE, EVENT publish)
        // NOTE: Currently dead code - ParserWorker does not send Raw/NostrEvent directly.
        // The intended flow is Engine → ParserWorker → CacheWorker → ConnectionsWorker.
        // This loop exists for future architecture changes and is tested but not exercised in production.
        let get_conn_parser = get_or_create_connection.clone();
        let connections_parser = self.connections.clone();
        let full_to_relay_parser = full_to_relay_sub_ids.clone();
        let relay_to_full_parser = relay_to_full_sub_ids.clone();
        let sub_relays_parser = sub_relays.clone();
        let sub_dedup_parser = sub_dedup.clone();
        spawn_worker(async move {
            info!("[ConnectionsWorker] parser loop started");
            loop {
                match from_parser.recv().await {
                    Ok(bytes) => {
                        let wm = match flatbuffers::root::<fb::WorkerMessage>(&bytes) {
                            Ok(w) => w,
                            Err(_) => {
                                warn!(
										"[ConnectionsWorker] Failed to decode WorkerMessage from parser"
									);
                                continue;
                            }
                        };
                        let url = wm.url().unwrap_or("");
                        match wm.content_type() {
                            fb::Message::Raw => {
                                if let Some(raw) = wm.content_as_raw() {
                                    let text = raw.raw();
                                    if !text.is_empty() && !url.is_empty() {
                                        let conn = get_conn_parser(url);
                                        let relay_text = encode_relay_frame(
                                            text,
                                            &full_to_relay_parser,
                                            &relay_to_full_parser,
                                        );
                                        let _ = conn.send_raw(&relay_text);
                                    } else if !text.is_empty() {
                                        if let Some((kind, sub_id)) = relay_frame_state(text) {
                                            if kind == "CLOSE" {
                                                let relays = sub_relays_parser
                                                    .borrow_mut()
                                                    .remove(&sub_id)
                                                    .unwrap_or_default();
                                                // Free the cross-relay dedup state for this subscription.
                                                sub_dedup_parser.borrow_mut().remove(&sub_id);
                                                for relay in relays {
                                                    let conn = get_conn_parser(&relay);
                                                    let relay_text = encode_relay_frame(
                                                        text,
                                                        &full_to_relay_parser,
                                                        &relay_to_full_parser,
                                                    );
                                                    let _ = conn.send_raw(&relay_text);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            fb::Message::NostrEvent => {
                                if let Some(ev) = wm.content_as_nostr_event() {
                                    let tags: Vec<serde_json::Value> = ev
                                        .tags()
                                        .iter()
                                        .map(|sv| {
                                            let arr: Vec<serde_json::Value> = sv
                                                .items()
                                                .map(|items| {
                                                    (0..items.len())
                                                        .map(|i| {
                                                            serde_json::Value::String(
                                                                items.get(i).to_string(),
                                                            )
                                                        })
                                                        .collect()
                                                })
                                                .unwrap_or_default();
                                            serde_json::Value::Array(arr)
                                        })
                                        .collect();
                                    let event_json = serde_json::json!({
                                        "id": ev.id(),
                                        "pubkey": ev.pubkey(),
                                        "kind": ev.kind(),
                                        "content": ev.content(),
                                        "tags": tags,
                                        "created_at": ev.created_at(),
                                        "sig": ev.sig(),
                                    });
                                    let frame = serde_json::json!(["EVENT", event_json]);
                                    if let Ok(text) = serde_json::to_string(&frame) {
                                        if !url.is_empty() {
                                            let conn = get_conn_parser(url);
                                            let _ = conn.send_raw(&text);
                                        }
                                    }
                                }
                            }
                            fb::Message::ConnectionStatus => {
                                if let Some(cs) = wm.content_as_connection_status() {
                                    match cs.status() {
                                        "CLOSE" => {
                                            if !url.is_empty() {
                                                let conn = get_conn_parser(url);
                                                let _ = conn.close();
                                                let mut map = connections_parser.write().unwrap();
                                                map.remove(url);
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    Err(_) => break,
                }
            }
            info!("[ConnectionsWorker] parser loop exiting");
        });

        // Loop for envelopes from cache (e.g. REQ frames)
        let get_conn_cache = get_or_create_connection.clone();
        let full_to_relay_cache = full_to_relay_sub_ids.clone();
        let relay_to_full_cache = relay_to_full_sub_ids.clone();
        let sub_relays_cache = sub_relays.clone();
        let sub_dedup_cache = sub_dedup.clone();
        spawn_worker(async move {
            info!("[ConnectionsWorker] cache loop started");
            loop {
                match from_cache.recv().await {
                    Ok(bytes) => {
                        send_envelope(
                            &bytes,
                            "cache",
                            &get_conn_cache,
                            &full_to_relay_cache,
                            &relay_to_full_cache,
                            &sub_relays_cache,
                            &sub_dedup_cache,
                        );
                    }
                    Err(_) => break,
                }
            }
            info!("[ConnectionsWorker] cache loop exiting");
        });

        // Loop for crypto traffic: NIP-46 relay envelopes and NIP-42 AUTH signed events.
        let connections_crypto = self.connections.clone();
        let get_conn_crypto = get_or_create_connection.clone();
        let full_to_relay_crypto = full_to_relay_sub_ids.clone();
        let relay_to_full_crypto = relay_to_full_sub_ids.clone();
        let sub_relays_crypto = sub_relays.clone();
        let sub_dedup_crypto = sub_dedup.clone();
        spawn_worker(async move {
            info!("[ConnectionsWorker] crypto loop started");
            loop {
                match from_crypto.recv().await {
                    Ok(bytes) => {
                        let resp = match flatbuffers::root::<fb::SignerResponse>(&bytes) {
                            Ok(r) => r,
                            Err(_) => {
                                send_envelope(
                                    &bytes,
                                    "crypto",
                                    &get_conn_crypto,
                                    &full_to_relay_crypto,
                                    &relay_to_full_crypto,
                                    &sub_relays_crypto,
                                    &sub_dedup_crypto,
                                );
                                continue;
                            }
                        };

                        let request_id = resp.request_id();
                        if request_id < 0x8000_0000_0000_0000 {
                            continue;
                        }

                        if let Some(result_str) = resp.result() {
                            if let Ok(parsed) =
                                serde_json::from_str::<serde_json::Value>(result_str)
                            {
                                let relay_url = parsed["relay"].as_str().unwrap_or("");
                                let event = parsed["event"].as_str().unwrap_or("");
                                if !relay_url.is_empty() && !event.is_empty() {
                                    let map = connections_crypto.read().unwrap();
                                    if let Some(conn) = map.get(relay_url) {
                                        conn.process_signed_auth(event);
                                    }
                                }
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            info!("[ConnectionsWorker] crypto loop exiting");
        });

        handle
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::channel::TokioWorkerChannel;
    use crate::generated::nostr::fb;
    use crate::traits::{RelayTransport, TransportError, TransportStatus};
    use crate::worker::batch_buffer::decode_conn_batch;
    use async_trait::async_trait;
    use std::collections::{HashMap, VecDeque};
    use std::sync::{Arc, Mutex, RwLock};
    use tokio::task::LocalSet;

    #[derive(Clone, Debug)]
    enum Call {
        Connect(String),
        Disconnect(String),
        Send(String, String),
    }

    #[derive(Clone)]
    struct MockRelayTransport {
        calls: Arc<Mutex<Vec<Call>>>,
        message_callbacks: Arc<RwLock<HashMap<String, Box<dyn Fn(String)>>>>,
        status_callbacks: Arc<RwLock<HashMap<String, Box<dyn Fn(TransportStatus)>>>>,
        connect_result: Arc<RwLock<Result<(), TransportError>>>,
        on_connect_callback: Arc<Mutex<Option<Box<dyn Fn(&MockRelayTransport)>>>>,
    }

    impl MockRelayTransport {
        fn new() -> Self {
            Self {
                calls: Arc::new(Mutex::new(Vec::new())),
                message_callbacks: Arc::new(RwLock::new(HashMap::new())),
                status_callbacks: Arc::new(RwLock::new(HashMap::new())),
                connect_result: Arc::new(RwLock::new(Ok(()))),
                on_connect_callback: Arc::new(Mutex::new(None)),
            }
        }

        fn set_connect_result(&self, result: Result<(), TransportError>) {
            *self.connect_result.write().unwrap() = result;
        }

        fn set_on_connect_callback(&self, callback: Box<dyn Fn(&MockRelayTransport)>) {
            *self.on_connect_callback.lock().unwrap() = Some(callback);
        }

        fn calls(&self) -> Vec<Call> {
            self.calls.lock().unwrap().clone()
        }

        fn invoke_message_callback(&self, url: &str, msg: String) {
            let cbs = self.message_callbacks.read().unwrap();
            if let Some(cb) = cbs.get(url) {
                cb(msg);
            }
        }

        fn invoke_status_callback(&self, url: &str, status: TransportStatus) {
            let cbs = self.status_callbacks.read().unwrap();
            if let Some(cb) = cbs.get(url) {
                cb(status);
            }
        }
    }

    #[async_trait(?Send)]
    impl RelayTransport for MockRelayTransport {
        async fn connect(&self, url: &str) -> Result<(), TransportError> {
            self.calls
                .lock()
                .unwrap()
                .push(Call::Connect(url.to_string()));
            if let Ok(cb_guard) = self.on_connect_callback.lock() {
                if let Some(cb) = cb_guard.as_ref() {
                    cb(self);
                }
            }
            self.connect_result.read().unwrap().clone()
        }

        fn disconnect(&self, url: &str) {
            self.calls
                .lock()
                .unwrap()
                .push(Call::Disconnect(url.to_string()));
        }

        async fn send(&self, url: &str, frame: String) -> Result<(), TransportError> {
            self.calls
                .lock()
                .unwrap()
                .push(Call::Send(url.to_string(), frame));
            Ok(())
        }

        fn on_message(&self, url: &str, callback: Box<dyn Fn(String)>) {
            self.message_callbacks
                .write()
                .unwrap()
                .insert(url.to_string(), callback);
        }

        fn on_status(&self, url: &str, callback: Box<dyn Fn(TransportStatus)>) {
            self.status_callbacks
                .write()
                .unwrap()
                .insert(url.to_string(), callback);
        }
    }

    fn build_raw_worker_message(url: &str, raw: &str) -> Vec<u8> {
        let mut fbb = flatbuffers::FlatBufferBuilder::new();
        let url_off = fbb.create_string(url);
        let raw_off = fbb.create_string(raw);
        let raw_msg = fb::Raw::create(&mut fbb, &fb::RawArgs { raw: Some(raw_off) });
        let wm = fb::WorkerMessage::create(
            &mut fbb,
            &fb::WorkerMessageArgs {
                sub_id: None,
                url: Some(url_off),
                type_: fb::MessageType::Raw,
                content_type: fb::Message::Raw,
                content: Some(raw_msg.as_union_value()),
            },
        );
        fbb.finish(wm, None);
        fbb.finished_data().to_vec()
    }

    fn build_nostr_event_worker_message(url: &str) -> Vec<u8> {
        let mut fbb = flatbuffers::FlatBufferBuilder::new();
        let url_off = fbb.create_string(url);

        let s1 = fbb.create_string("p");
        let s2 = fbb.create_string("pubkey1");
        let tag1_items = fbb.create_vector(&[s1, s2]);
        let tag1 = fb::StringVec::create(
            &mut fbb,
            &fb::StringVecArgs {
                items: Some(tag1_items),
            },
        );
        let tags = fbb.create_vector(&[tag1]);

        let id_off = fbb.create_string("event_id_123");
        let pubkey_off = fbb.create_string("pubkey_123");
        let content_off = fbb.create_string("hello world");
        let sig_off = fbb.create_string("sig_123");

        let event = fb::NostrEvent::create(
            &mut fbb,
            &fb::NostrEventArgs {
                id: Some(id_off),
                pubkey: Some(pubkey_off),
                kind: 1,
                content: Some(content_off),
                tags: Some(tags),
                created_at: 1234567890,
                sig: Some(sig_off),
            },
        );

        let wm = fb::WorkerMessage::create(
            &mut fbb,
            &fb::WorkerMessageArgs {
                sub_id: None,
                url: Some(url_off),
                type_: fb::MessageType::NostrEvent,
                content_type: fb::Message::NostrEvent,
                content: Some(event.as_union_value()),
            },
        );
        fbb.finish(wm, None);
        fbb.finished_data().to_vec()
    }

    fn build_close_worker_message(url: &str) -> Vec<u8> {
        let mut fbb = flatbuffers::FlatBufferBuilder::new();
        let url_off = fbb.create_string(url);
        let status_off = fbb.create_string("CLOSE");
        let cs = fb::ConnectionStatus::create(
            &mut fbb,
            &fb::ConnectionStatusArgs {
                relay_url: Some(url_off),
                status: Some(status_off),
                message: None,
            },
        );
        let wm = fb::WorkerMessage::create(
            &mut fbb,
            &fb::WorkerMessageArgs {
                sub_id: None,
                url: Some(url_off),
                type_: fb::MessageType::ConnectionStatus,
                content_type: fb::Message::ConnectionStatus,
                content: Some(cs.as_union_value()),
            },
        );
        fbb.finish(wm, None);
        fbb.finished_data().to_vec()
    }

    async fn setup() -> (
        Arc<MockRelayTransport>,
        ConnectionsHandle,
        TokioWorkerChannel,
        TokioWorkerChannel,
        TokioWorkerChannel,
        TokioWorkerChannel,
    ) {
        let (parser_test, parser_worker) = TokioWorkerChannel::new_pair();
        let (parser_out_worker, parser_out_test) = TokioWorkerChannel::new_pair();
        let (cache_test, cache_worker) = TokioWorkerChannel::new_pair();
        let (crypto_test, crypto_worker) = TokioWorkerChannel::new_pair();
        let crypto_sender = crypto_worker.clone_sender();

        let transport = Arc::new(MockRelayTransport::new());
        let worker = ConnectionsWorker::new(transport.clone());

        let handle = worker.run(
            Box::new(parser_worker),
            parser_out_worker.clone_sender(),
            Box::new(cache_worker),
            Box::new(crypto_worker),
            crypto_sender,
        );

        (
            transport,
            handle,
            parser_test,
            parser_out_test,
            cache_test,
            crypto_test,
        )
    }

    #[tokio::test]
    async fn test_parser_raw_message_sent_to_transport() {
        let local = LocalSet::new();
        local
				.run_until(async {
                    let (transport, _worker, parser_test, _parser_out_test, _cache_test, _crypto_test) = setup().await;

					let msg = build_raw_worker_message("wss://r", "hello");
					parser_test.send(&msg).await.unwrap();
					tokio::task::yield_now().await;
					tokio::task::yield_now().await;
					tokio::task::yield_now().await;

					let calls = transport.calls();
					assert!(
						calls.iter().any(|c| matches!(c, Call::Connect(url) if url == "wss://r")),
						"connect was not called"
					);
					assert!(
						calls.iter().any(|c| matches!(c, Call::Send(url, frame) if url == "wss://r" && frame == "hello")),
						"send was not called with correct frame"
					);
				})
				.await;
    }

    #[tokio::test]
    async fn test_parser_nostr_event_publishes_json_event() {
        let local = LocalSet::new();
        local
            .run_until(async {
                let (transport, _worker, parser_test, _parser_out_test, _cache_test, _crypto_test) =
                    setup().await;

                let msg = build_nostr_event_worker_message("wss://r");
                parser_test.send(&msg).await.unwrap();
                tokio::task::yield_now().await;
                tokio::task::yield_now().await;
                tokio::task::yield_now().await;

                let calls = transport.calls();
                let send_call = calls
                    .iter()
                    .find(|c| matches!(c, Call::Send(url, _) if url == "wss://r"))
                    .expect("send was not called");
                if let Call::Send(_, frame) = send_call {
                    let parsed: serde_json::Value = serde_json::from_str(frame).unwrap();
                    let arr = parsed.as_array().unwrap();
                    assert_eq!(arr.len(), 2);
                    assert_eq!(arr[0], "EVENT");
                    let event = &arr[1];
                    assert_eq!(event["id"], "event_id_123");
                    assert_eq!(event["pubkey"], "pubkey_123");
                    assert_eq!(event["kind"], 1);
                    assert_eq!(event["content"], "hello world");
                    assert_eq!(event["created_at"], 1234567890);
                    assert_eq!(event["sig"], "sig_123");
                    assert!(event["tags"].is_array());
                }
            })
            .await;
    }

    #[tokio::test]
    async fn test_parser_close_disconnects() {
        let local = LocalSet::new();
        local
            .run_until(async {
                let (transport, _worker, parser_test, _parser_out_test, _cache_test, _crypto_test) =
                    setup().await;

                let msg = build_close_worker_message("wss://r");
                parser_test.send(&msg).await.unwrap();
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;

                let calls = transport.calls();
                assert!(
                    calls
                        .iter()
                        .any(|c| matches!(c, Call::Disconnect(url) if url == "wss://r")),
                    "disconnect was not called"
                );
            })
            .await;
    }

    #[tokio::test]
    async fn test_cache_envelope_forwards_req_frames() {
        let local = LocalSet::new();
        local
				.run_until(async {
					let (transport, _worker, _parser_test, _parser_out_test, cache_test, _crypto_test) = setup().await;

					let envelope = serde_json::json!({
						"relays": ["wss://r"],
						"frames": [r#"["REQ","s1",{}]"#]
					});
					let bytes = serde_json::to_vec(&envelope).unwrap();
					cache_test.send(&bytes).await.unwrap();
					tokio::task::yield_now().await;
					tokio::task::yield_now().await;
					tokio::task::yield_now().await;

					let calls = transport.calls();
					assert!(
						calls.iter().any(|c| matches!(c, Call::Connect(url) if url == "wss://r")),
						"connect was not called"
					);
					assert!(
						calls.iter().any(|c| matches!(c, Call::Send(url, frame) if url == "wss://r" && frame == r#"["REQ","s1",{}]"#)),
						"send was not called with correct frame"
					);
				})
				.await;
    }

    #[tokio::test]
    async fn test_parser_close_fans_out_to_subscription_relays() {
        let local = LocalSet::new();
        local
			.run_until(async {
				let (transport, _worker, parser_test, _parser_out_test, cache_test, _crypto_test) =
					setup().await;

				let envelope = serde_json::json!({
					"relays": ["wss://r1", "wss://r2"],
					"frames": [r#"["REQ","s1",{}]"#]
				});
				let bytes = serde_json::to_vec(&envelope).unwrap();
				cache_test.send(&bytes).await.unwrap();
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;

				let msg = build_raw_worker_message("", r#"["CLOSE","s1"]"#);
				parser_test.send(&msg).await.unwrap();
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;

				let calls = transport.calls();
				assert!(
					calls
						.iter()
						.any(|c| matches!(c, Call::Send(url, frame) if url == "wss://r1" && frame == r#"["CLOSE","s1"]"#)),
					"r1 did not receive CLOSE"
				);
				assert!(
					calls
						.iter()
						.any(|c| matches!(c, Call::Send(url, frame) if url == "wss://r2" && frame == r#"["CLOSE","s1"]"#)),
					"r2 did not receive CLOSE"
				);
			})
			.await;
    }

    #[test]
    fn test_long_subscription_ids_are_encoded_for_relay_and_decoded_for_app() {
        let full_to_relay = Rc::new(RefCell::new(HashMap::<String, String>::new()));
        let relay_to_full = Rc::new(RefCell::new(HashMap::<String, String>::new()));
        let full_sub_id = format!("f_{}_{}", "a".repeat(64), "counter");
        let frame = serde_json::json!(["REQ", full_sub_id, {}]).to_string();

        let relay_frame = encode_relay_frame(&frame, &full_to_relay, &relay_to_full);
        let parsed: serde_json::Value = serde_json::from_str(&relay_frame).unwrap();
        let relay_sub_id = parsed[1].as_str().unwrap();

        assert!(relay_sub_id.len() < 64);
        assert_ne!(relay_sub_id, full_sub_id);
        assert_eq!(
            decode_relay_sub_id(relay_sub_id, &relay_to_full),
            full_sub_id
        );
    }

    #[tokio::test]
    async fn test_wake_all_wakes_existing_connections() {
        let local = LocalSet::new();
        local
            .run_until(async {
                let (transport, handle, _parser_test, _parser_out_test, cache_test, _crypto_test) =
                    setup().await;

                let req_frame = r#"["REQ","s1",{}]"#;
                let envelope = serde_json::json!({
                    "relays": ["wss://r"],
                    "frames": [req_frame]
                });
                let bytes = serde_json::to_vec(&envelope).unwrap();
                cache_test.send(&bytes).await.unwrap();
                tokio::task::yield_now().await;
                tokio::task::yield_now().await;
                tokio::task::yield_now().await;

                handle.wake_all();
                tokio::task::yield_now().await;
                tokio::task::yield_now().await;
                tokio::task::yield_now().await;
                tokio::task::yield_now().await;

                let calls = transport.calls();
                let disconnects = calls
                    .iter()
                    .filter(|c| matches!(c, Call::Disconnect(url) if url == "wss://r"))
                    .count();
                let sends = calls
                    .iter()
                    .filter(|c| matches!(c, Call::Send(url, frame) if url == "wss://r" && frame == req_frame))
                    .count();

                assert!(disconnects >= 1, "wake_all should wake existing relay connections");
                assert!(sends >= 2, "wake_all should replay active relay REQ frames");
            })
            .await;
    }

    #[tokio::test]
    async fn test_crypto_envelope_forwards_nip46_frames() {
        let local = LocalSet::new();
        local
            .run_until(async {
                let (transport, _worker, _parser_test, _parser_out_test, _cache_test, crypto_test) =
                    setup().await;

                let envelope = serde_json::json!({
                    "relays": ["wss://r"],
                    "frames": [r#"["REQ","n46:client",{"kinds":[24133]}]"#]
                });
                let bytes = serde_json::to_vec(&envelope).unwrap();
                crypto_test.send(&bytes).await.unwrap();
                tokio::task::yield_now().await;
                tokio::task::yield_now().await;
                tokio::task::yield_now().await;

                let calls = transport.calls();
                assert!(
                    calls
                        .iter()
                        .any(|c| matches!(c, Call::Connect(url) if url == "wss://r")),
                    "connect was not called"
                );
                assert!(
                    calls.iter().any(|c| {
                        matches!(
                            c,
                            Call::Send(url, frame)
                                if url == "wss://r"
                                    && frame == r#"["REQ","n46:client",{"kinds":[24133]}]"#
                        )
                    }),
                    "NIP-46 frame was not sent"
                );
            })
            .await;
    }

    #[tokio::test]
    async fn test_nip46_relay_message_routes_to_crypto() {
        let local = LocalSet::new();
        local
            .run_until(async {
                let (
                    transport,
                    _worker,
                    parser_test,
                    mut parser_out_test,
                    _cache_test,
                    mut crypto_test,
                ) = setup().await;

                let trigger = build_raw_worker_message("wss://r", "trigger");
                parser_test.send(&trigger).await.unwrap();
                tokio::task::yield_now().await;
                tokio::task::yield_now().await;

                let _ = parser_out_test.recv().await.unwrap();
                let _ = parser_out_test.recv().await.unwrap();
                let _ = parser_out_test.recv().await.unwrap();

                transport.invoke_message_callback(
                    "wss://r",
                    r#"["EVENT","n46:client",{"kind":24133}]"#.to_string(),
                );

                let bytes = crypto_test
                    .recv()
                    .await
                    .expect("expected NIP-46 frame on crypto");
                let wm = flatbuffers::root::<fb::WorkerMessage>(&bytes).unwrap();
                assert_eq!(wm.sub_id(), Some("n46:client"));

                assert!(
                    tokio::time::timeout(
                        std::time::Duration::from_millis(10),
                        parser_out_test.recv()
                    )
                    .await
                    .is_err(),
                    "NIP-46 frame should not be routed to parser"
                );
            })
            .await;
    }

    #[tokio::test]
    async fn test_reconnect_failure_does_not_drop_frames() {
        let local = LocalSet::new();
        local
            .run_until(async {
                let (transport, _worker, _parser_test, _parser_out_test, cache_test, _crypto_test) =
                    setup().await;
                transport.set_connect_result(Err(TransportError::Other("fail".to_string())));

                let envelope = serde_json::json!({
                    "relays": ["wss://r"],
                    "frames": [r#"["REQ","s1",{}]"#]
                });
                let bytes = serde_json::to_vec(&envelope).unwrap();
                cache_test.send(&bytes).await.unwrap();
                tokio::task::yield_now().await;
                tokio::task::yield_now().await;

                let calls = transport.calls();
                assert!(
                    calls
                        .iter()
                        .any(|c| matches!(c, Call::Connect(url) if url == "wss://r")),
                    "connect was not called"
                );
                // With RelayConnection, failed initial connect means the queue drainer
                // never starts, so frames are queued but not sent until reconnect succeeds.
                assert!(
                    !calls
                        .iter()
                        .any(|c| matches!(c, Call::Send(url, _) if url == "wss://r")),
                    "send should not be attempted when initial connect fails"
                );
            })
            .await;
    }

    #[tokio::test]
    async fn test_transport_message_callback_builds_worker_message() {
        let local = LocalSet::new();
        local
            .run_until(async {
                let (
                    transport,
                    _worker,
                    parser_test,
                    mut parser_out_test,
                    _cache_test,
                    _crypto_test,
                ) = setup().await;

                // Trigger callback registration by sending any message for the URL
                let trigger = build_raw_worker_message("wss://r", "trigger");
                parser_test.send(&trigger).await.unwrap();
                tokio::task::yield_now().await;
                tokio::task::yield_now().await;
                // Drain synthetic status messages from RelayConnection initial connect
                let _ = parser_out_test.recv().await.unwrap(); // "connecting" (new)
                let _ = parser_out_test.recv().await.unwrap(); // "connecting" (connect)
                let _ = parser_out_test.recv().await.unwrap(); // "connected" or "failed"

                // Invoke the stored message callback
                transport.invoke_message_callback("wss://r", r#"["EVENT","sub1",{}]"#.to_string());

                // EVENT frames travel in the compact envelope: the raw event
                // object JSON, batched, no WorkerMessage wrapper.
                let mut pending = VecDeque::new();
                let payload = recv_event_payload(&mut parser_out_test, &mut pending).await;
                assert_eq!(payload, "{}", "event object mismatch");
            })
            .await;
    }

    #[tokio::test]
    async fn test_transport_status_callback() {
        let local = LocalSet::new();
        local
            .run_until(async {
                let (
                    transport,
                    _worker,
                    parser_test,
                    mut parser_out_test,
                    _cache_test,
                    _crypto_test,
                ) = setup().await;

                // Trigger callback registration by sending any message for the URL
                let trigger = build_raw_worker_message("wss://r", "trigger");
                parser_test.send(&trigger).await.unwrap();
                tokio::task::yield_now().await;
                tokio::task::yield_now().await;

                // Drain synthetic status messages from RelayConnection initial connect
                let _ = parser_out_test.recv().await.unwrap(); // "connecting" (new)
                let _ = parser_out_test.recv().await.unwrap(); // "connecting" (connect)
                let _ = parser_out_test.recv().await.unwrap(); // "connected" or "failed"

                // Invoke the stored status callback
                transport.invoke_status_callback(
                    "wss://r",
                    TransportStatus::Connected {
                        url: "wss://r".to_string(),
                    },
                );

                let bytes = parser_out_test.recv().await.unwrap();
                let wm = flatbuffers::root::<fb::WorkerMessage>(&bytes).unwrap();
                assert_eq!(
                    wm.content_type(),
                    fb::Message::ConnectionStatus,
                    "expected ConnectionStatus message"
                );
            })
            .await;
    }

    #[tokio::test]
    async fn test_disconnect_during_active_subscription() {
        let local = LocalSet::new();
        local
            .run_until(async {
                let (
                    transport,
                    _worker,
                    parser_test,
                    mut parser_out_test,
                    _cache_test,
                    _crypto_test,
                ) = setup().await;

                // Create active connection/registration for URL "wss://r1"
                let trigger = build_raw_worker_message("wss://r1", "trigger");
                parser_test.send(&trigger).await.unwrap();
                tokio::task::yield_now().await;
                tokio::task::yield_now().await;

                // Drain synthetic status messages from RelayConnection initial connect
                let _ = parser_out_test.recv().await.unwrap(); // "connecting" (new)
                let _ = parser_out_test.recv().await.unwrap(); // "connecting" (connect)
                let _ = parser_out_test.recv().await.unwrap(); // "connected" or "failed"

                // Verify callbacks are set up by invoking them
                transport.invoke_message_callback("wss://r1", r#"["EVENT","sub1",{}]"#.to_string());
                let mut pending = VecDeque::new();
                let payload = recv_event_payload(&mut parser_out_test, &mut pending).await;
                assert_eq!(payload, "{}", "callback should work before disconnect");

                // Send ConnectionStatus with status="CLOSE" to trigger disconnect
                let close_msg = build_close_worker_message("wss://r1");
                parser_test.send(&close_msg).await.unwrap();
                tokio::task::yield_now().await;

                // Verify transport.disconnect("wss://r1") was called
                let calls = transport.calls();
                assert!(
                    calls
                        .iter()
                        .any(|c| matches!(c, Call::Disconnect(url) if url == "wss://r1")),
                    "disconnect was not called for wss://r1"
                );
            })
            .await;
    }

    #[tokio::test]
    async fn test_reconnect_resumes_sending() {
        let local = LocalSet::new();
        local
				.run_until(async {
					let (transport, _worker, parser_test, _parser_out_test, cache_test, _crypto_test) = setup().await;

					// Connect and register URL "wss://r1" via cache envelope (which calls connect)
					let envelope = serde_json::json!({
						"relays": ["wss://r1"],
						"frames": [r#"["REQ","s1",{}]"#]
					});
					let bytes = serde_json::to_vec(&envelope).unwrap();
					cache_test.send(&bytes).await.unwrap();
					tokio::task::yield_now().await;
					tokio::task::yield_now().await;
					tokio::task::yield_now().await;

					// Verify connect was called
					let calls_before = transport.calls();
					assert!(
						calls_before.iter().any(|c| matches!(c, Call::Connect(url) if url == "wss://r1")),
						"connect was not called initially"
					);

					// Disconnect it
					let close_msg = build_close_worker_message("wss://r1");
					parser_test.send(&close_msg).await.unwrap();
					tokio::task::yield_now().await;

					// Clear calls to check for new ones
					transport.calls.lock().unwrap().clear();

					// Re-register by sending via cache again (get_or_create returns existing connection)
					let envelope2 = serde_json::json!({
						"relays": ["wss://r1"],
						"frames": [r#"["REQ","s2",{}]"#]
					});
					let bytes2 = serde_json::to_vec(&envelope2).unwrap();
					cache_test.send(&bytes2).await.unwrap();
					tokio::task::yield_now().await;
					tokio::task::yield_now().await;

					// Verify new frames can be sent after reconnect
					// Note: connect won't be called again because RelayConnection is reused,
					// but send should still work
					let calls_after = transport.calls();
					assert!(
						calls_after.iter().any(|c| matches!(c, Call::Send(url, frame) if url == "wss://r1" && frame == r#"["REQ","s2",{}]"#)),
						"send was not called with new frame after reconnect"
					);
				})
				.await;
    }

    #[tokio::test]
    async fn test_multiple_relays_one_fails() {
        let local = LocalSet::new();
        local
            .run_until(async {
                let (transport, _worker, _parser_test, _parser_out_test, cache_test, _crypto_test) =
                    setup().await;

                // Make all connects fail (MockRelayTransport uses a single shared result)
                transport.set_connect_result(Err(TransportError::Other("fail".to_string())));

                // Send an envelope with 3 relays
                let envelope = serde_json::json!({
                    "relays": ["wss://r1", "wss://r2", "wss://r3"],
                    "frames": [r#"["REQ","s1",{}]"#]
                });
                let bytes = serde_json::to_vec(&envelope).unwrap();
                cache_test.send(&bytes).await.unwrap();
                tokio::task::yield_now().await;
                tokio::task::yield_now().await;

                // Verify all 3 relays attempted connect
                let calls = transport.calls();
                assert!(
                    calls
                        .iter()
                        .any(|c| matches!(c, Call::Connect(url) if url == "wss://r1")),
                    "r1 connect was not attempted"
                );
                assert!(
                    calls
                        .iter()
                        .any(|c| matches!(c, Call::Connect(url) if url == "wss://r2")),
                    "r2 connect was not attempted"
                );
                assert!(
                    calls
                        .iter()
                        .any(|c| matches!(c, Call::Connect(url) if url == "wss://r3")),
                    "r3 connect was not attempted"
                );

                // With RelayConnection, failed initial connect means queue drainer never starts,
                // so frames are queued but not sent. No Send calls are expected here.
                assert!(
                    !calls.iter().any(|c| matches!(c, Call::Send(_, _))),
                    "no sends should occur when all initial connects fail"
                );
            })
            .await;
    }

    #[tokio::test]
    async fn test_transport_error_callback_propagation() {
        let local = LocalSet::new();
        local
            .run_until(async {
                let (
                    transport,
                    _worker,
                    parser_test,
                    mut parser_out_test,
                    _cache_test,
                    _crypto_test,
                ) = setup().await;

                // Register a URL with on_status callback by sending any message
                let trigger = build_raw_worker_message("wss://r1", "trigger");
                parser_test.send(&trigger).await.unwrap();
                tokio::task::yield_now().await;
                tokio::task::yield_now().await;

                // Drain synthetic status messages from RelayConnection initial connect
                let _ = parser_out_test.recv().await.unwrap(); // "connecting" (new)
                let _ = parser_out_test.recv().await.unwrap(); // "connecting" (connect)
                let _ = parser_out_test.recv().await.unwrap(); // "connected" or "failed"

                // Invoke the callback with TransportStatus::Failed
                transport.invoke_status_callback(
                    "wss://r1",
                    TransportStatus::Failed {
                        url: "wss://r1".to_string(),
                    },
                );

                // Verify the callback captures the failed status
                let bytes = parser_out_test.recv().await.unwrap();
                let wm = flatbuffers::root::<fb::WorkerMessage>(&bytes).unwrap();

                // The status callback should serialize and send ConnectionStatus bytes
                assert_eq!(
                    wm.content_type(),
                    fb::Message::ConnectionStatus,
                    "expected ConnectionStatus message for failed status"
                );

                // Verify it's a ConnectionStatus with "failed" status
                if let Some(cs) = wm.content_as_connection_status() {
                    assert_eq!(cs.relay_url(), "wss://r1", "relay_url mismatch");
                    assert_eq!(cs.status(), "failed", "status should be 'failed'");
                } else {
                    panic!("Expected ConnectionStatus content");
                }
            })
            .await;
    }

    /// One frame unwrapped from a parser-channel payload.
    enum ParserFrame {
        /// Raw EVENT JSON object (compact-envelope batch frame).
        Raw(String),
        /// WorkerMessage FlatBuffer bytes (bare singles and WM batches).
        Wm(Vec<u8>),
    }

    /// Unwrap one channel payload into frames: a magic-prefixed
    /// connections→parser batch yields all its frames; anything else is a
    /// bare single WorkerMessage (relay status / control path).
    fn unwrap_parser_payload(bytes: &[u8], pending: &mut VecDeque<ParserFrame>) {
        match decode_conn_batch(bytes) {
            Some(batch) => {
                for (_sid, data) in batch.frames {
                    if batch.raw_events {
                        pending.push_back(ParserFrame::Raw(
                            String::from_utf8(data).expect("raw event is utf8"),
                        ));
                    } else {
                        pending.push_back(ParserFrame::Wm(data));
                    }
                }
            }
            None => pending.push_back(ParserFrame::Wm(bytes.to_vec())),
        }
    }

    /// Next WorkerMessage from the parser channel, transparently decoding
    /// batched payloads and skipping raw EVENT frames.
    async fn recv_worker_message(
        rx: &mut TokioWorkerChannel,
        pending: &mut VecDeque<ParserFrame>,
    ) -> Vec<u8> {
        loop {
            while let Some(frame) = pending.pop_front() {
                if let ParserFrame::Wm(bytes) = frame {
                    return bytes;
                }
            }
            let bytes = rx.recv().await.expect("parser channel closed");
            unwrap_parser_payload(&bytes, pending);
        }
    }

    async fn recv_event_payload(
        rx: &mut TokioWorkerChannel,
        pending: &mut VecDeque<ParserFrame>,
    ) -> String {
        // Read until an EVENT payload arrives (raw batch frame or legacy Raw
        // WorkerMessage), skipping ConnectionStatus and other control messages.
        loop {
            while let Some(frame) = pending.pop_front() {
                match frame {
                    ParserFrame::Raw(json) => return json,
                    ParserFrame::Wm(bytes) => {
                        let wm = flatbuffers::root::<fb::WorkerMessage>(&bytes).unwrap();
                        if wm.content_type() == fb::Message::Raw {
                            if let Some(raw) = wm.content_as_raw() {
                                return raw.raw().to_string();
                            }
                        }
                    }
                }
            }
            let bytes = match tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv())
                .await
            {
                Ok(Ok(b)) => b,
                _ => panic!("timed out waiting for EVENT message"),
            };
            unwrap_parser_payload(&bytes, pending);
        }
    }

    async fn expect_no_event(rx: &mut TokioWorkerChannel, pending: &mut VecDeque<ParserFrame>) {
        // Assert no EVENT frame arrives within a short window.
        for frame in pending.drain(..) {
            match frame {
                ParserFrame::Raw(_) => panic!("duplicate EVENT was forwarded to parser"),
                ParserFrame::Wm(bytes) => {
                    let wm = flatbuffers::root::<fb::WorkerMessage>(&bytes).unwrap();
                    assert_ne!(
                        wm.content_type(),
                        fb::Message::Raw,
                        "duplicate EVENT was forwarded to parser"
                    );
                }
            }
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(50);
        loop {
            let now = std::time::Instant::now();
            if now >= deadline {
                return;
            }
            match tokio::time::timeout(deadline - now, rx.recv()).await {
                Ok(Ok(bytes)) => {
                    let mut frames = VecDeque::new();
                    unwrap_parser_payload(&bytes, &mut frames);
                    for frame in frames {
                        match frame {
                            ParserFrame::Raw(_) => {
                                panic!("duplicate EVENT was forwarded to parser")
                            }
                            ParserFrame::Wm(bytes) => {
                                let wm = flatbuffers::root::<fb::WorkerMessage>(&bytes).unwrap();
                                assert_ne!(
                                    wm.content_type(),
                                    fb::Message::Raw,
                                    "duplicate EVENT was forwarded to parser"
                                );
                            }
                        }
                    }
                }
                _ => return,
            }
        }
    }

    fn event_frame(sub_id: &str, id_hex: &str) -> String {
        format!(
            r#"["EVENT","{}",{{"id":"{}","pubkey":"pk","kind":1,"content":"hi","tags":[],"created_at":1,"sig":"s"}}]"#,
            sub_id, id_hex
        )
    }

    #[tokio::test]
    async fn test_cross_relay_dedup_same_sub_forwards_once() {
        let local = LocalSet::new();
        local
            .run_until(async {
                let (transport, _worker, _parser_test, mut parser_out_test, cache_test, _crypto_test) =
                    setup().await;

                let id_hex = "ab".repeat(32);
                let envelope = serde_json::json!({
                    "relays": ["wss://r1", "wss://r2"],
                    "frames": [r#"["REQ","s1",{}]"#]
                });
                cache_test.send(&serde_json::to_vec(&envelope).unwrap()).await.unwrap();
                for _ in 0..5 {
                    tokio::task::yield_now().await;
                }

                let mut pending = VecDeque::new();
                // Same event id, same sub, from relay 1: forwarded
                transport.invoke_message_callback("wss://r1", event_frame("s1", &id_hex));
                let payload = recv_event_payload(&mut parser_out_test, &mut pending).await;
                assert!(payload.contains(&id_hex), "first arrival should be forwarded");

                // Same event id, same sub, from relay 2: suppressed
                transport.invoke_message_callback("wss://r2", event_frame("s1", &id_hex));
                expect_no_event(&mut parser_out_test, &mut pending).await;
            })
            .await;
    }

    #[tokio::test]
    async fn test_cross_relay_dedup_different_subs_both_delivered() {
        let local = LocalSet::new();
        local
            .run_until(async {
                let (transport, _worker, _parser_test, mut parser_out_test, cache_test, _crypto_test) =
                    setup().await;

                let id_hex = "cd".repeat(32);
                let envelope = serde_json::json!({
                    "relays": ["wss://r1"],
                    "frames": [r#"["REQ","s1",{}]"#, r#"["REQ","s2",{}]"#]
                });
                cache_test.send(&serde_json::to_vec(&envelope).unwrap()).await.unwrap();
                for _ in 0..5 {
                    tokio::task::yield_now().await;
                }

                // Same event id under two different subs: both delivered
                let mut pending = VecDeque::new();
                transport.invoke_message_callback("wss://r1", event_frame("s1", &id_hex));
                let first = recv_event_payload(&mut parser_out_test, &mut pending).await;
                assert!(first.contains(&id_hex));

                transport.invoke_message_callback("wss://r1", event_frame("s2", &id_hex));
                let second = recv_event_payload(&mut parser_out_test, &mut pending).await;
                assert!(second.contains(&id_hex));
            })
            .await;
    }

    #[tokio::test]
    async fn test_cross_relay_dedup_state_freed_on_close() {
        let local = LocalSet::new();
        local
            .run_until(async {
                let (transport, _worker, parser_test, mut parser_out_test, cache_test, _crypto_test) =
                    setup().await;

                let id_hex = "ef".repeat(32);
                let envelope = serde_json::json!({
                    "relays": ["wss://r1"],
                    "frames": [r#"["REQ","s1",{}]"#]
                });
                cache_test.send(&serde_json::to_vec(&envelope).unwrap()).await.unwrap();
                for _ in 0..5 {
                    tokio::task::yield_now().await;
                }

                transport.invoke_message_callback("wss://r1", event_frame("s1", &id_hex));
                let mut pending = VecDeque::new();
                let _ = recv_event_payload(&mut parser_out_test, &mut pending).await;

                // Duplicate is suppressed while the sub is open
                transport.invoke_message_callback("wss://r1", event_frame("s1", &id_hex));
                expect_no_event(&mut parser_out_test, &mut pending).await;

                // CLOSE the subscription (parser close path): dedup state is freed
                let close_msg = build_raw_worker_message("", r#"["CLOSE","s1"]"#);
                parser_test.send(&close_msg).await.unwrap();
                for _ in 0..5 {
                    tokio::task::yield_now().await;
                }

                // Same id under a fresh subscription is delivered again
                transport.invoke_message_callback("wss://r1", event_frame("s1", &id_hex));
                let payload = recv_event_payload(&mut parser_out_test, &mut pending).await;
                assert!(payload.contains(&id_hex), "event should be forwarded after CLOSE freed dedup state");
            })
            .await;
    }

    #[tokio::test]
    async fn test_auth_event_full_flow() {
        let local = LocalSet::new();
        local
			.run_until(async {
				let (transport, _worker, _parser_test, _parser_out_test, cache_test, mut crypto_test) = setup().await;

				// Send cache envelope with REQ frame to establish connection
				let envelope = serde_json::json!({
					"relays": ["wss://r"],
					"frames": [r#"["REQ","s1",{}]"#]
				});
				let bytes = serde_json::to_vec(&envelope).unwrap();
				cache_test.send(&bytes).await.unwrap();

				// Let workers run and connection establish
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;

				// Now inject AUTH challenge from relay (after on_message is registered)
				transport.invoke_message_callback(
					"wss://r",
					r#"["AUTH","challenge123"]"#.to_string(),
				);
				tokio::task::yield_now().await;

				// Read SignerRequest from crypto channel
				let crypto_bytes = crypto_test.recv().await.expect("expected crypto request");
				let req = flatbuffers::root::<fb::SignerRequest>(&crypto_bytes).unwrap();
				assert_eq!(req.op(), fb::SignerOp::AuthEvent, "expected AuthEvent op");
				let payload = req.payload().expect("expected payload");
				let parsed: serde_json::Value = serde_json::from_str(payload).unwrap();
				assert_eq!(parsed["challenge"], "challenge123", "challenge mismatch");
				assert_eq!(parsed["relay"], "wss://r", "relay mismatch");
				let request_id = req.request_id();

				// Build SignerResponse with signed event
				let result_json = serde_json::json!({
					"event": r#"{"id":"abc","pubkey":"pk","created_at":123,"kind":22242,"tags":[["challenge","challenge123"],["relay","wss://r"]],"content":"","sig":"sig"}"#,
					"relay": "wss://r"
				})
				.to_string();

				let mut fbb = flatbuffers::FlatBufferBuilder::new();
				let result_off = fbb.create_string(&result_json);
				let resp = fb::SignerResponse::create(
					&mut fbb,
					&fb::SignerResponseArgs {
						request_id,
						result: Some(result_off),
						error: None,
					},
				);
				fbb.finish(resp, None);
				crypto_test.send(fbb.finished_data()).await.unwrap();

				// Let workers process the signed auth response
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;

				// Verify AUTH frame was sent to relay
				let calls = transport.calls();
				let auth_sent = calls.iter().any(|c| {
					matches!(c, Call::Send(url, frame) if url == "wss://r" && frame.starts_with(r#"["AUTH",{"#))
				});
				assert!(auth_sent, "expected AUTH frame to be sent to relay");

				// Inject OK response from relay
				transport.invoke_message_callback(
					"wss://r",
					r#"["OK","auth-id","true"]"#.to_string(),
				);

				// Let auth handling run
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;

				// Verify original REQ frame was sent
				let req_sent = calls.iter().any(|c| {
					matches!(c, Call::Send(url, frame) if url == "wss://r" && frame == r#"["REQ","s1",{}]"#)
				});
				assert!(req_sent, "expected original REQ frame to be sent");
			})
			.await;
    }

    /// Collect `n` EVENT payloads from the parser channel, transparently
    /// unwrapping batched payloads and skipping bare status/control messages.
    async fn recv_n_events(
        rx: &mut TokioWorkerChannel,
        n: usize,
    ) -> Vec<(String, Vec<u8>)> {
        let mut events: Vec<(String, Vec<u8>)> = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(2000);
        while events.len() < n {
            let now = std::time::Instant::now();
            assert!(now < deadline, "timed out waiting for {n} events");
            let bytes = tokio::time::timeout(deadline - now, rx.recv())
                .await
                .expect("timed out waiting for events")
                .expect("parser channel closed");
            if let Some(batch) = decode_conn_batch(&bytes) {
                events.extend(batch.frames);
            }
            // Bare single messages are relay statuses / control frames: skip.
        }
        events
    }

    #[tokio::test]
    async fn test_event_frames_are_batched_to_parser() {
        let local = LocalSet::new();
        local
            .run_until(async {
                let (transport, _worker, _parser_test, mut parser_out_test, cache_test, _crypto_test) =
                    setup().await;

                let envelope = serde_json::json!({
                    "relays": ["wss://r1"],
                    "frames": [r#"["REQ","s1",{}]"#]
                });
                cache_test.send(&serde_json::to_vec(&envelope).unwrap()).await.unwrap();
                for _ in 0..5 {
                    tokio::task::yield_now().await;
                }

                // Three distinct events, same sub, in one synchronous burst:
                // they must arrive inside batched channel messages, in order.
                let ids: Vec<String> = (1u8..=3).map(|i| format!("{:02x}", i).repeat(32)).collect();
                for id in &ids {
                    transport.invoke_message_callback("wss://r1", event_frame("s1", id));
                }

                let batched = recv_n_events(&mut parser_out_test, 3).await;
                assert_eq!(batched.len(), 3);
                for (i, (sid, data)) in batched.iter().enumerate() {
                    assert_eq!(sid, "s1");
                    let json = std::str::from_utf8(data).expect("raw event is utf8");
                    assert!(json.contains(&ids[i]), "frame {} out of order", i);
                }
            })
            .await;
    }

    #[tokio::test]
    async fn test_control_frame_flushes_pending_events_first() {
        let local = LocalSet::new();
        local
            .run_until(async {
                let (transport, _worker, _parser_test, mut parser_out_test, cache_test, _crypto_test) =
                    setup().await;

                let envelope = serde_json::json!({
                    "relays": ["wss://r1"],
                    "frames": [r#"["REQ","s1",{}]"#]
                });
                cache_test.send(&serde_json::to_vec(&envelope).unwrap()).await.unwrap();
                for _ in 0..5 {
                    tokio::task::yield_now().await;
                }

                // A buffered EVENT followed by EOSE: the EOSE must flush the
                // pending event batch ahead of itself and travel unbatched.
                let id_hex = "0a".repeat(32);
                transport.invoke_message_callback("wss://r1", event_frame("s1", &id_hex));
                transport.invoke_message_callback("wss://r1", r#"["EOSE","s1"]"#.to_string());

                let mut pending = VecDeque::new();
                let event = recv_event_payload(&mut parser_out_test, &mut pending).await;
                assert!(event.contains(&id_hex), "event must arrive before EOSE");

                // The EOSE is the next message after the flushed batch, as a
                // bare (unbatched) ConnectionStatus WorkerMessage.
                let bytes = recv_worker_message(&mut parser_out_test, &mut pending).await;
                let wm = flatbuffers::root::<fb::WorkerMessage>(&bytes).unwrap();
                assert_eq!(wm.sub_id(), Some("s1"));
                let cs = wm.content_as_connection_status().expect("EOSE as ConnectionStatus");
                assert_eq!(cs.status(), "EOSE");
            })
            .await;
    }

    #[tokio::test]
    async fn test_batch_size_threshold_flushes_large_burst() {
        let local = LocalSet::new();
        local
            .run_until(async {
                let (transport, _worker, _parser_test, mut parser_out_test, cache_test, _crypto_test) =
                    setup().await;

                let envelope = serde_json::json!({
                    "relays": ["wss://r1"],
                    "frames": [r#"["REQ","s1",{}]"#]
                });
                cache_test.send(&serde_json::to_vec(&envelope).unwrap()).await.unwrap();
                for _ in 0..5 {
                    tokio::task::yield_now().await;
                }

                // 20 events x ~1.5KB content: the 16KB size threshold flushes
                // mid-burst, so the first channel payload is already a large
                // batch; all 20 frames arrive across batches, in order.
                let big_content = "x".repeat(1500);
                let mut sent = Vec::new();
                for i in 1u8..=20 {
                    let id = format!("{:02x}", i).repeat(32);
                    let frame = format!(
                        r#"["EVENT","s1",{{"id":"{}","pubkey":"pk","kind":1,"content":"{}","tags":[],"created_at":1,"sig":"s"}}]"#,
                        id, big_content
                    );
                    sent.push(id);
                    transport.invoke_message_callback("wss://r1", frame);
                }

                // First payload must be a batch payload near the 16KB threshold
                // (the burst is synchronous, so no timer flush can interleave).
                let first = loop {
                    let bytes = tokio::time::timeout(
                        std::time::Duration::from_millis(500),
                        parser_out_test.recv(),
                    )
                    .await
                    .expect("timed out waiting for first batch")
                    .expect("parser channel closed");
                    if decode_conn_batch(&bytes).is_some() {
                        break bytes;
                    }
                };
                assert!(
                    first.len() >= 12 * 1024,
                    "first flush should be threshold-sized, got {} bytes",
                    first.len()
                );

                let mut events = decode_conn_batch(&first).unwrap().frames;
                let rest = recv_n_events(&mut parser_out_test, 20 - events.len()).await;
                events.extend(rest);
                assert_eq!(events.len(), 20);
                for (i, (_sid, data)) in events.iter().enumerate() {
                    let json = std::str::from_utf8(data).expect("raw event is utf8");
                    assert!(json.contains(&sent[i]), "frame {} out of order", i);
                }
            })
            .await;
    }

    #[tokio::test]
    async fn test_malformed_event_frame_takes_worker_message_path() {
        let local = LocalSet::new();
        local
            .run_until(async {
                let (transport, _worker, _parser_test, mut parser_out_test, cache_test, _crypto_test) =
                    setup().await;

                let envelope = serde_json::json!({
                    "relays": ["wss://r1"],
                    "frames": [r#"["REQ","s1",{}]"#]
                });
                cache_test.send(&serde_json::to_vec(&envelope).unwrap()).await.unwrap();
                for _ in 0..5 {
                    tokio::task::yield_now().await;
                }

                // EVENT frame without an event object: forwarded untouched as
                // a bare WorkerMessage so the parser handles it exactly as before.
                transport.invoke_message_callback("wss://r1", r#"["EVENT","s1"]"#.to_string());

                let mut pending = VecDeque::new();
                let payload = recv_event_payload(&mut parser_out_test, &mut pending).await;
                assert_eq!(payload, r#"["EVENT","s1"]"#);
            })
            .await;
    }
}
