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
            // Produce one job per WorkerLine, on demand.
            let job_stream = stream::unfold((rings, subs), |(rings, subs)| async move {
                loop {
                    if let Some(bytes) = rings.read_out() {
                        match flatbuffers::root::<fb::WorkerLine>(&bytes) {
                            Ok(line) => {
                                // We construct a job future for each line, capturing only owned data.
                                if let Some(sub_id) = line.sub_id() {
                                    let (pipeline_arc, buffer, eosed, publish_id) = {
                                        let guard = subs.read().unwrap();
                                        if let Some(sub) = guard.get(sub_id) {
                                            (
                                                sub.pipeline.clone(),
                                                sub.buffer.clone(),
                                                sub.eosed,
                                                sub.publish_id.clone(),
                                            )
                                        } else {
                                            // Unknown subscription id; skip to next message
                                            continue;
                                        }
                                    };

                                    // Owned values for the job
                                    let sub_id_owned = sub_id.to_owned();
                                    let url_owned = line.relay().url().to_owned();

                                    let job: LocalBoxFuture<'static, ()> = match line.kind() {
                                        fb::MsgKind::Event => {
                                            // Prepare owned event payload if present
                                            let raw_str =
                                                match std::str::from_utf8(line.raw().bytes()) {
                                                    Ok(s) => s.to_owned(),
                                                    Err(e) => {
                                                        info!("Invalid UTF-8 in raw: {}", e);
                                                        // No job to run; try next message
                                                        continue;
                                                    }
                                                };

                                            // Extract the event payload; if missing, skip
                                            let event_raw = match extract_first_three(&raw_str)
                                                .and_then(|parts| parts[2].map(|s| s.to_owned()))
                                            {
                                                Some(v) => v,
                                                None => {
                                                    // No event payload; try next message
                                                    continue;
                                                }
                                            };

                                            let pipeline_arc = pipeline_arc.clone();
                                            let buffer = buffer.clone();
                                            let eosed_flag = eosed;
                                            Box::pin(async move {
                                                let mut pipeline = pipeline_arc.lock().await;
                                                if let Ok(Some(output)) =
                                                    pipeline.process(&event_raw).await
                                                {
                                                    if SharedBufferManager::write_to_buffer(
                                                        &buffer, &output,
                                                    )
                                                    .await
                                                    .is_ok()
                                                    {
                                                        if eosed_flag {
                                                            post_worker_message(
                                                                &JsValue::from_str(&sub_id_owned),
                                                            );
                                                        }
                                                    }
                                                }
                                            })
                                        }
                                        fb::MsgKind::Eose => {
                                            let buffer = buffer.clone();
                                            let subs = subs.clone();
                                            Box::pin(async move {
                                                SharedBufferManager::send_connection_status(
                                                    &buffer, &url_owned, "EOSE", "",
                                                )
                                                .await;
                                                post_worker_message(&JsValue::from_str(
                                                    &sub_id_owned,
                                                ));
                                                {
                                                    let mut guard = subs.write().unwrap();
                                                    if let Some(sub) = guard.get_mut(&sub_id_owned)
                                                    {
                                                        sub.eosed = true;
                                                    }
                                                }
                                            })
                                        }
                                        fb::MsgKind::Ok => {
                                            let buffer = buffer.clone();
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
                                        fb::MsgKind::Notice => Box::pin(async move {
                                            info!("Received notice: {:?}", &url_owned);
                                        }),
                                        fb::MsgKind::Closed => Box::pin(async move {
                                            info!(
                                                "Sub closed {}, on relay {:?}",
                                                &sub_id_owned, &url_owned
                                            );
                                        }),
                                        fb::MsgKind::Auth => Box::pin(async move {
                                            info!("Auth needed on relay: {:?}", &url_owned);
                                        }),
                                        _ => Box::pin(async move {
                                            info!("Unknown WorkerLine kind");
                                        }),
                                    };

                                    // Yield one job; the stream will be polled again when a slot frees up.
                                    return Some((job, (rings, subs)));
                                } else {
                                    // No sub_id; skip
                                    continue;
                                }
                            }
                            Err(e) => {
                                info!("Invalid WorkerLine: {}", e);
                                // Skip and try to read the next one
                                continue;
                            }
                        }
                    } else {
                        // No data currently available; brief sleep to avoid busy spinning
                        TimeoutFuture::new(8).await;
                        // Try reading again
                        continue;
                    }
                }
            });

            // Execute up to 3 jobs at a time. A new job is pulled only when one finishes.
            job_stream.for_each_concurrent(3, |job| job).await;
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
