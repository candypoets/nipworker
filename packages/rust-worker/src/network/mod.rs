pub mod cache_processor;
pub mod interfaces;
pub mod publish;
pub mod subscription;

use crate::generated::nostr::fb;
use crate::nostr::Template;
use crate::parser::Parser;
use crate::relays::ClientMessage;
use crate::types::network::Request;
use crate::utils::buffer::SharedBufferManager;
use crate::utils::js_interop::post_worker_message;
use crate::utils::json::extract_first_three;
use crate::utils::sab_ring::WsRings;
use crate::NostrError;
use crate::{db::NostrDB, pipeline::Pipeline};
use futures::lock::Mutex;
use gloo_timers::future::TimeoutFuture;
use js_sys::SharedArrayBuffer;
use rustc_hash::FxHashMap;
use std::rc::Rc;
use std::sync::{Arc, RwLock};
use tracing::info;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::spawn_local;

type Result<T> = std::result::Result<T, NostrError>;

// Parsed view of a single out frame
struct ParsedOut {
    url: String,
    kind: String,            // uppercased
    sub_id: Option<String>,  // cleaned (no quotes)
    payload: Option<String>, // third element when present (e.g., event json)
}

struct Sub {
    pipeline: Arc<Mutex<Pipeline>>,
    buffer: SharedArrayBuffer,
    relay_urls: Vec<String>,
    eosed: bool,
    publish_id: Option<String>,
}

pub struct NetworkManager {
    rings: Rc<WsRings>,
    publish_manager: publish::PublishManager,
    subscription_manager: subscription::SubscriptionManager,
    subscriptions: Arc<RwLock<FxHashMap<String, Sub>>>,
}

// Fast, zero-allocation unquote: removes a single pair of "..." if present.
// Assumes no escaped quotes at the ends (which is true for standard JSON tokens).
fn unquote_simple(s: &str) -> &str {
    let b = s.as_bytes();
    if b.len() >= 2 && b.first() == Some(&b'"') && b.last() == Some(&b'"') {
        &s[1..b.len() - 1]
    } else {
        s
    }
}

impl NetworkManager {
    pub fn new(database: Arc<NostrDB>, parser: Arc<Parser>, rings: WsRings) -> Self {
        let publish_manager = publish::PublishManager::new(database.clone(), parser.clone());

        let subscription_manager =
            subscription::SubscriptionManager::new(database.clone(), parser.clone());

        let manager = Self {
            rings: Rc::new(rings),
            publish_manager,
            subscription_manager,
            subscriptions: Arc::new(RwLock::new(FxHashMap::default())),
        };

        manager.start_out_ring_reader();
        manager
    }

    // Parse [u16 url_len][url][u32 raw_len][raw] and shallow extract kind/sub_id/payload
    fn parse_out_frame(bytes: &[u8]) -> Option<ParsedOut> {
        if bytes.len() < 2 {
            return None;
        }
        let url_len = u16::from_be_bytes([bytes[0], bytes[1]]) as usize;
        let mut off = 2usize;
        if bytes.len() < off + url_len + 4 {
            return None;
        }

        let url = std::str::from_utf8(&bytes[off..off + url_len])
            .ok()?
            .to_owned();
        off += url_len;

        if bytes.len() < off + 4 {
            return None;
        }
        let raw_len =
            u32::from_be_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]])
                as usize;
        off += 4;

        if bytes.len() < off + raw_len {
            return None;
        }
        let raw_str = std::str::from_utf8(&bytes[off..off + raw_len])
            .ok()?
            .to_owned();

        let parts = extract_first_three(&raw_str)?;
        let kind_tok_raw = parts[0].unwrap_or("");
        let kind = unquote_simple(kind_tok_raw).to_ascii_uppercase();
        let sub_id = parts[1].map(unquote_simple).map(|s| s.to_owned());
        let payload = parts[2].map(|s| s.to_owned());

        Some(ParsedOut {
            url,
            kind,
            sub_id,
            payload,
        })
    }

    async fn handle_message_core(subs: Arc<RwLock<FxHashMap<String, Sub>>>, parsed: ParsedOut) {
        let ParsedOut {
            url,
            kind,
            sub_id,
            payload,
        } = parsed;

        match kind.as_str() {
            "EVENT" => {
                let sub_id = match sub_id {
                    Some(s) => s,
                    None => {
                        info!("missing sub_id for EVENT");
                        return;
                    }
                };
                let event_raw = match payload {
                    Some(p) => p,
                    None => return,
                };

                // Resolve sub context
                let (pipeline_arc, buffer, eosed_flag) = {
                    let guard = subs.read().unwrap();
                    match guard.get(&sub_id) {
                        Some(sub) => (sub.pipeline.clone(), sub.buffer.clone(), sub.eosed),
                        None => {
                            info!("unknown subId: {}", sub_id);
                            return;
                        }
                    }
                };

                let mut pipeline = pipeline_arc.lock().await;
                if let Ok(Some(output)) = pipeline.process(&event_raw).await {
                    if SharedBufferManager::write_to_buffer(&buffer, &output)
                        .await
                        .is_ok()
                    {
                        if eosed_flag {
                            post_worker_message(&JsValue::from_str(&sub_id));
                        }
                    }
                }
            }
            "EOSE" => {
                let sub_id = match sub_id {
                    Some(s) => s,
                    None => {
                        info!("missing sub_id for EOSE");
                        return;
                    }
                };

                let buffer = {
                    let guard = subs.read().unwrap();
                    match guard.get(&sub_id) {
                        Some(sub) => sub.buffer.clone(),
                        None => {
                            info!("unknown subId (EOSE): {}", sub_id);
                            return;
                        }
                    }
                };

                SharedBufferManager::send_connection_status(&buffer, &url, "EOSE", "").await;
                post_worker_message(&JsValue::from_str(&sub_id));

                {
                    let mut guard = subs.write().unwrap();
                    if let Some(sub) = guard.get_mut(&sub_id) {
                        sub.eosed = true;
                    }
                }
            }
            "OK" => {
                let sub_id = match sub_id {
                    Some(s) => s,
                    None => {
                        info!("missing sub_id for OK");
                        return;
                    }
                };

                let (buffer, publish_id) = {
                    let guard = subs.read().unwrap();
                    match guard.get(&sub_id) {
                        Some(sub) => (sub.buffer.clone(), sub.publish_id.clone()),
                        None => {
                            info!("unknown subId (OK): {}", sub_id);
                            return;
                        }
                    }
                };
                SharedBufferManager::send_connection_status(
                    &buffer,
                    &url,
                    payload.as_deref().unwrap_or(""),
                    "",
                )
                .await;
                if let Some(pub_id) = publish_id {
                    post_worker_message(&JsValue::from_str(&pub_id));
                }
            }
            "NOTICE" => {
                info!("Received notice: {:?}", &url);
            }
            "CLOSED" => {
                let sid = sub_id.unwrap_or_default();
                info!("Sub closed {}, on relay {:?}", &sid, &url);
            }
            "AUTH" => {
                info!("Auth needed on relay: {:?}", &url);
            }
            _ => {
                // ignore unknown kinds
            }
        }
    }

    fn start_out_ring_reader(&self) {
        use futures::{channel::mpsc, FutureExt, StreamExt};
        use std::cell::Cell;

        let rings = self.rings.clone();
        let subs = self.subscriptions.clone();

        // Single-threaded inflight counter
        let inflight: Rc<Cell<usize>> = Rc::new(Cell::new(0));
        let max_inflight = 6usize;

        // Channel to notify when a slot becomes available
        let (slot_tx, mut slot_rx) = mpsc::unbounded::<()>();

        spawn_local({
            let inflight = inflight.clone();
            let slot_tx_main = slot_tx.clone();
            async move {
                // Initial delay to stagger startup
                TimeoutFuture::new(500).await;

                // Backoffs for fairness and avoiding spin
                let mut empty_backoff_ms: u32 = 8;
                let mut full_backoff_ms: u32 = 8;

                loop {
                    // Throttle when at capacity: wait for either a slot or timeout
                    if inflight.get() >= max_inflight {
                        let mut timeout = TimeoutFuture::new(full_backoff_ms).fuse();
                        let mut slot = slot_rx.next().fuse();
                        futures::select! {
                            _ = timeout => {
                                full_backoff_ms = (full_backoff_ms.saturating_mul(2)).min(512);
                            }
                            _ = slot => {
                                // slot freed, try immediately
                                full_backoff_ms = 8;
                            }
                        }
                        continue;
                    }

                    if let Some(bytes) = rings.read_out() {
                        // Progress: reset empty backoff
                        empty_backoff_ms = 8;

                        if let Some(parsed) = Self::parse_out_frame(&bytes) {
                            // Fast-path filter before spawning to avoid extra tasks
                            match parsed.kind.as_str() {
                                // Purely logging — handle inline, do not spawn
                                "NOTICE" => {
                                    info!("Received notice: {:?}", &parsed.url);
                                    continue;
                                }
                                "AUTH" => {
                                    info!("Auth needed on relay: {:?}", &parsed.url);
                                    continue;
                                }
                                "CLOSED" => {
                                    let sid = parsed.sub_id.clone().unwrap_or_default();
                                    info!("Sub closed {}, on relay {:?}", &sid, &parsed.url);
                                    continue;
                                }
                                // Require sub_id and existing subscription; skip if unknown
                                "EVENT" | "EOSE" | "OK" => {
                                    let Some(ref sid) = parsed.sub_id else {
                                        continue;
                                    };
                                    let has_sub = {
                                        let g = subs.read().unwrap();
                                        g.contains_key(sid)
                                    };
                                    if !has_sub {
                                        // Unknown sub — drop early, don't spawn
                                        continue;
                                    }
                                    // Fall through to spawn
                                }
                                // Unknown kinds — ignore
                                _ => {
                                    continue;
                                }
                            }

                            // Reserve a slot and spawn the async handler
                            inflight.set(inflight.get() + 1);
                            let inflight_clone = inflight.clone();
                            let subs_clone = subs.clone();
                            let slot_tx = slot_tx_main.clone();
                            let parsed_for_task = parsed;

                            spawn_local(async move {
                                Self::handle_message_core(subs_clone, parsed_for_task).await;
                                inflight_clone.set(inflight_clone.get().saturating_sub(1));
                                let _ = slot_tx.unbounded_send(());
                            });

                            // small yield for fairness
                            TimeoutFuture::new(0).await;

                            // Reset full backoff as we made progress
                            full_backoff_ms = 8;
                        } else {
                            // malformed frame — tiny yield
                            TimeoutFuture::new(0).await;
                        }
                    } else {
                        // Ring empty — exponential backoff
                        TimeoutFuture::new(empty_backoff_ms).await;
                        empty_backoff_ms = (empty_backoff_ms.saturating_mul(2)).min(512);
                    }
                }
            }
        });
    }

    pub async fn open_subscription(
        &self,
        subscription_id: String,
        shared_buffer: SharedArrayBuffer,
        requests: &Vec<fb::Request<'_>>,
        config: &fb::SubscriptionConfig<'_>,
    ) -> Result<()> {
        // early bailout if the sub already exist
        if self
            .subscriptions
            .read()
            .unwrap()
            .contains_key(&subscription_id)
        {
            return Ok(());
        }

        let parsed_requests: Vec<Request> = requests
            .iter()
            .map(|request| Request::from_flatbuffer(request))
            .collect();

        let (pipeline, relay_filters) = self
            .subscription_manager
            .process_subscription(
                &subscription_id,
                shared_buffer.clone(),
                parsed_requests,
                config,
            )
            .await?;

        self.subscriptions.write().unwrap().insert(
            subscription_id.clone(),
            Sub {
                pipeline: Arc::new(Mutex::new(pipeline)),
                buffer: shared_buffer.clone(),
                eosed: false,
                relay_urls: relay_filters.keys().cloned().collect(),
                publish_id: None,
            },
        );

        // Construct and write one REQ frame per relay group:
        // ["REQ", subscription_id, ...filters]
        for (relay_url, filters) in relay_filters {
            let req_message = ClientMessage::req(subscription_id.clone(), filters);

            let frame = req_message.to_json()?;
            let relays = [relay_url.as_str()];
            let frames = [frame.clone()];
            // info!(
            //     "Writing REQ frame '{}' to relay: {}",
            //     frame.clone(),
            //     relay_url
            // );
            // // Write JSON envelope { relays: [...], frames: [...] } to the inRing.
            // // Use an unsafe mutable borrow to avoid changing struct mutability here.
            let _ = self.rings.write_in_envelope(&relays, &frames);
        }

        Ok(())
    }

    pub async fn close_subscription(&self, subscription_id: String) -> Result<()> {
        if let Some(sub) = self.subscriptions.read().unwrap().get(&subscription_id) {
            // Write a CLOSE frame to each relay
            for relay_url in &sub.relay_urls {
                let close_message = ClientMessage::close(subscription_id.clone());
                let frame = close_message.to_json()?;
                let relays = [relay_url.as_str()];
                let frames = [frame.clone()];
                let _ = self.rings.write_in_envelope(&relays, &frames);
            }
        }

        // Remove the subscription from the map
        self.subscriptions.write().unwrap().remove(&subscription_id);

        Ok(())
    }

    pub async fn publish_event(
        &self,
        publish_id: String,
        template: &Template,
        shared_buffer: SharedArrayBuffer,
    ) -> Result<()> {
        let (event, relays) = self
            .publish_manager
            .publish_event(publish_id.clone(), template)
            .await?;

        self.subscriptions.write().unwrap().insert(
            event.id.to_string(),
            Sub {
                pipeline: Arc::new(Mutex::new(Pipeline::new(vec![], "".to_string()).unwrap())),
                buffer: shared_buffer.clone(),
                eosed: false,
                relay_urls: relays.clone(),
                publish_id: Some(publish_id.clone()),
            },
        );

        for relay_url in &relays {
            let event_message = ClientMessage::event(event.clone());
            let frame = event_message.to_json()?;
            let relays_array = [relay_url.as_str()];
            let frames = [frame];
            let _ = self.rings.write_in_envelope(&relays_array, &frames);
        }

        Ok(())
    }
}
