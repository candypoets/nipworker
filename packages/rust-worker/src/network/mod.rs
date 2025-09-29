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
        let rings = self.rings.clone();
        let subs = self.subscriptions.clone();

        spawn_local(async move {
            loop {
                // Drain as many committed records as available this tick
                let mut processed = 0;
                while let Some(bytes) = rings.read_out() {
                    match flatbuffers::root::<fb::WorkerLine>(&bytes) {
                        Ok(line) => {
                            if let Some(sub_id) = line.sub_id() {
                                let (pipeline_arc, buffer, eosed) = {
                                    let guard = subs.read().unwrap();
                                    if let Some(sub) = guard.get(sub_id) {
                                        (sub.pipeline.clone(), sub.buffer.clone(), sub.eosed)
                                    } else {
                                        // Unknown subscription id; skip
                                        continue;
                                    }
                                };
                                match line.kind() {
                                    fb::MsgKind::Event => {
                                        let raw_str = match std::str::from_utf8(line.raw().bytes())
                                        {
                                            Ok(s) => s,
                                            Err(e) => {
                                                info!("Invalid UTF-8 in raw: {}", e);
                                                continue;
                                            }
                                        };

                                        if let Some(parts) = extract_first_three(&raw_str) {
                                            if let Some(event_raw) = parts[2] {
                                                let mut pipeline = pipeline_arc.lock().await;
                                                if let Ok(Some(output)) =
                                                    pipeline.process(event_raw).await
                                                {
                                                    if SharedBufferManager::write_to_buffer(
                                                        &buffer, &output,
                                                    )
                                                    .await
                                                    .is_ok()
                                                    {
                                                        if eosed {
                                                            post_worker_message(
                                                                &JsValue::from_str(&sub_id),
                                                            );
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    fb::MsgKind::Notice => {
                                        info!("Received notice: {:?}", line.relay().url());
                                    }
                                    fb::MsgKind::Eose => {
                                        SharedBufferManager::send_connection_status(
                                            &buffer,
                                            line.relay().url(),
                                            "EOSE",
                                            "",
                                        )
                                        .await;
                                        post_worker_message(&JsValue::from_str(&sub_id));
                                        {
                                            let mut guard = subs.write().unwrap();
                                            if let Some(sub) = guard.get_mut(sub_id) {
                                                sub.eosed = true;
                                            }
                                        }
                                    }
                                    fb::MsgKind::Ok => {
                                        info!("Publish {} OK", &sub_id);
                                    }
                                    fb::MsgKind::Closed => {
                                        info!(
                                            "Sub closed {}, on relay {:?}",
                                            &sub_id,
                                            line.relay().url()
                                        );
                                    }
                                    fb::MsgKind::Auth => {
                                        info!("Auth needed on relay: {:?}", line.relay().url());
                                    }
                                    _ => {
                                        info!("Unknown WorkerLine kind");
                                    }
                                }
                            }

                            // TODO: route to your pipeline or parser here
                        }
                        Err(e) => {
                            info!("Invalid WorkerLine: {}", e);
                        }
                    }

                    processed += 1;
                    if processed >= 64 {
                        break;
                    }
                }

                // Small sleep to avoid busy spinning
                TimeoutFuture::new(if processed > 0 { 1 } else { 8 }).await;
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
        info!("Opening subscription: {}", subscription_id);

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
                info!(
                    "Writing CLOSE frame '{}' to relay: {}",
                    frame.clone(),
                    relay_url
                );
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
            .publish_event(publish_id, template, shared_buffer)
            .await?;

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
