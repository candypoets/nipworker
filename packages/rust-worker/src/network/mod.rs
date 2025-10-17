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

    fn start_out_ring_reader(&self) {
        use futures::{future::LocalBoxFuture, stream, StreamExt};

        let rings = self.rings.clone();
        let subs = self.subscriptions.clone();

        // Drive a stream of jobs, executed with at most 3 in parallel.
        spawn_local(async move {
            // Initial delay to stagger startup
            TimeoutFuture::new(500).await;
            // Produce one job per WorkerLine, on demand.
            let job_stream = stream::unfold((rings, subs), |(rings, subs)| async move {
                loop {
                    if let Some(bytes) = rings.read_out() {
                        // Decode tiny header: [u16 url_len][url][u32 raw_len][raw] (big-endian)
                        if bytes.len() < 2 {
                            // Not enough bytes to read url_len
                            continue;
                        }
                        let url_len = u16::from_be_bytes([bytes[0], bytes[1]]) as usize;
                        let mut off = 2usize;

                        if bytes.len() < off + url_len + 4 {
                            // Not enough bytes for url + raw_len
                            continue;
                        }

                        // URL
                        let url_bytes = &bytes[off..off + url_len];
                        off += url_len;
                        let url_owned = match std::str::from_utf8(url_bytes) {
                            Ok(s) => s.to_owned(),
                            Err(e) => {
                                info!("Invalid UTF-8 in url: {}", e);
                                continue;
                            }
                        };

                        // RAW length and bytes
                        let raw_len = u32::from_be_bytes([
                            bytes[off],
                            bytes[off + 1],
                            bytes[off + 2],
                            bytes[off + 3],
                        ]) as usize;
                        off += 4;

                        if bytes.len() < off + raw_len {
                            info!("invalid payload");
                            // Not enough bytes for raw payload
                            continue;
                        }
                        let raw_bytes = &bytes[off..off + raw_len];

                        // Convert raw to &str (json array like ["EVENT","sub", {...}] etc.)
                        let raw_str = match std::str::from_utf8(raw_bytes) {
                            Ok(s) => s.to_owned(),
                            Err(e) => {
                                info!("Invalid UTF-8 in raw: {}", e);
                                continue;
                            }
                        };

                        // Shallow-parse kind and subId using your existing helper.
                        // Expected: parts[0] = kind string, parts[1] = subId (if present), parts[2] = payload (if present)
                        let parts = match extract_first_three(&raw_str) {
                            Some(p) => p,
                            None => {
                                info!("invalid array");
                                // Not a valid top-level array
                                continue;
                            }
                        };

                        // After: let parts = match extract_first_three(&raw_str) { ... };

                        // Clean tokens returned by the shallow scanner (they include surrounding quotes)
                        let kind_tok_raw = parts[0].unwrap_or("");
                        let kind_tok = unquote_simple(kind_tok_raw);

                        let sub_id_raw = match parts[1] {
                            Some(s) => s,
                            None => {
                                // If you want to handle NOTICE/AUTH/etc. without sub_id, branch here instead of continue.
                                continue;
                            }
                        };
                        let sub_id_clean = unquote_simple(sub_id_raw);

                        // Resolve subscription context using cleaned sub_id
                        let (pipeline_arc, buffer, eosed, publish_id) = {
                            let guard = subs.read().unwrap();
                            if let Some(sub) = guard.get(sub_id_clean) {
                                (
                                    sub.pipeline.clone(),
                                    sub.buffer.clone(),
                                    sub.eosed,
                                    sub.publish_id.clone(),
                                )
                            } else {
                                // Log cleaned vs raw for diagnostics
                                info!(
                                    "unknown subId: {} (raw token: {})",
                                    sub_id_clean, sub_id_raw
                                );
                                continue;
                            }
                        };

                        // Now branch on cleaned kind
                        let url_owned = url_owned; // already owned from earlier
                        let sub_id_owned = sub_id_clean.to_owned();

                        let job: LocalBoxFuture<'static, ()> = match kind_tok
                            .to_ascii_uppercase()
                            .as_str()
                        {
                            "EVENT" => {
                                // Extract the event payload (3rd element) as before
                                let event_raw = match extract_first_three(&raw_str)
                                    .and_then(|p| p[2].map(|s| s.to_owned()))
                                {
                                    Some(v) => v,
                                    None => continue,
                                };

                                let pipeline_arc = pipeline_arc.clone();
                                let buffer = buffer.clone();
                                let eosed_flag = eosed;
                                Box::pin(async move {
                                    let mut pipeline = pipeline_arc.lock().await;
                                    if let Ok(Some(output)) = pipeline.process(&event_raw).await {
                                        if SharedBufferManager::write_to_buffer(&buffer, &output)
                                            .await
                                            .is_ok()
                                        {
                                            if eosed_flag {
                                                post_worker_message(&JsValue::from_str(
                                                    &sub_id_owned,
                                                ));
                                            }
                                        }
                                    }
                                })
                            }
                            "EOSE" => {
                                let buffer = buffer.clone();
                                let url_owned = url_owned.clone();
                                let sub_id_owned = sub_id_owned.clone();
                                let subs = subs.clone();
                                Box::pin(async move {
                                    SharedBufferManager::send_connection_status(
                                        &buffer, &url_owned, "EOSE", "",
                                    )
                                    .await;
                                    post_worker_message(&JsValue::from_str(&sub_id_owned));
                                    {
                                        let mut guard = subs.write().unwrap();
                                        if let Some(sub) = guard.get_mut(&sub_id_owned) {
                                            sub.eosed = true;
                                        }
                                    }
                                })
                            }
                            "OK" => {
                                let buffer = buffer.clone();
                                let url_owned = url_owned.clone();
                                let publish_id = publish_id.clone();
                                Box::pin(async move {
                                    SharedBufferManager::send_connection_status(
                                        &buffer, &url_owned, "OK", "",
                                    )
                                    .await;
                                    if let Some(ref pub_id) = publish_id {
                                        post_worker_message(&JsValue::from_str(pub_id));
                                    }
                                })
                            }
                            "NOTICE" => {
                                let url_owned = url_owned.clone();
                                Box::pin(async move {
                                    info!("Received notice: {:?}", &url_owned);
                                })
                            }
                            "CLOSED" => {
                                let url_owned = url_owned.clone();
                                let sub_id_owned = sub_id_owned.clone();
                                Box::pin(async move {
                                    info!(
                                        "Sub closed {}, on relay {:?}",
                                        &sub_id_owned, &url_owned
                                    );
                                })
                            }
                            "AUTH" => {
                                let url_owned = url_owned.clone();
                                Box::pin(async move {
                                    info!("Auth needed on relay: {:?}", &url_owned);
                                })
                            }
                            _ => Box::pin(async move {
                                // info!(
                                //     "Unknown message kind in raw payload (cleaned kind: {})",
                                //     kind_tok
                                // );
                            }),
                        };

                        // Yield one job; the stream will be polled again when a slot frees up.
                        TimeoutFuture::new(0).await;
                        return Some((job, (rings, subs)));
                    } else {
                        // No data currently available; brief sleep to avoid busy spinning
                        TimeoutFuture::new(64).await;
                        continue;
                    }
                }
            });

            // Execute up to 3 jobs at a time. A new job is pulled only when one finishes.
            job_stream.for_each_concurrent(6, |job| job).await;
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
