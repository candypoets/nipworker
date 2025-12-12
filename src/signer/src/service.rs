use gloo_timers::future::TimeoutFuture;
use shared::{generated::nostr::fb, SabRing};
use std::{cell::RefCell, rc::Rc};
use tracing::{info, warn};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

/// ServiceLoop is the background driver for the signer:
/// - drains signer_service_request and writes signer_service_response
/// - can also observe ws_response_signer (connections -> signer) as needed
///
/// This is a minimal placeholder that proves the SAB pipes and structure.
/// Integrate FlatBuffers decode/encode and actual signer dispatch in a follow-up.
pub struct ServiceLoop {
    // Parser <-> Signer rings (SPSC)
    svc_req: Rc<RefCell<SabRing>>,
    svc_resp: Rc<RefCell<SabRing>>,

    // Signer <-> Connections rings (SPSC)
    ws_req: Rc<RefCell<SabRing>>,
    ws_resp: Rc<RefCell<SabRing>>,
}

impl ServiceLoop {
    /// Accepts SAB-backed rings. The `active` parameter is intentionally generic and unused
    /// here so the caller can pass any active signer state without us depending on its type.
    pub fn new<T>(
        svc_req: Rc<RefCell<SabRing>>,
        svc_resp: Rc<RefCell<SabRing>>,
        ws_req: Rc<RefCell<SabRing>>,
        ws_resp: Rc<RefCell<SabRing>>,
        _active: T,
    ) -> Self {
        Self {
            svc_req,
            svc_resp,
            ws_req,
            ws_resp,
        }
    }

    /// Spawns the service loops.
    /// - service loop: echoes back any incoming payload to prove the pipe
    /// - nip46 pump: logs inbound frames from connections (placeholder)
    pub fn start(&mut self) {
        self.spawn_service_loop();
        self.spawn_nip46_pump();
        info!("[signer][service] loops started");
    }

    fn spawn_service_loop(&self) {
        let svc_req = self.svc_req.clone();
        let svc_resp = self.svc_resp.clone();

        spawn_local(async move {
            let mut sleep_ms: u32 = 8;
            let max_sleep: u32 = 256;

            loop {
                let maybe = { svc_req.borrow_mut().read_next() };

                if let Some(bytes) = maybe {
                    sleep_ms = 8;

                    // Decode FlatBuffer SignerRequest and build FlatBuffer SignerResponse
                    match flatbuffers::root::<fb::SignerRequest>(&bytes) {
                        Ok(req) => {
                            let rid = req.request_id();
                            let payload = req.payload_json().unwrap_or("");
                            // For now, echo payload as result_json to validate the pipe
                            let res_str = if payload.is_empty() { "{}" } else { payload };

                            let mut fbb = flatbuffers::FlatBufferBuilder::new();
                            let res_off = fbb.create_string(res_str);
                            let resp = fb::SignerResponse::create(
                                &mut fbb,
                                &fb::SignerResponseArgs {
                                    request_id: rid,
                                    ok: true,
                                    result_json: Some(res_off),
                                    error: None,
                                },
                            );
                            fbb.finish(resp, None);
                            let out = fbb.finished_data();
                            let ok = svc_resp.borrow_mut().write(out);
                            if !ok {
                                warn!("[signer][service] svc_resp ring full, response dropped");
                            }
                        }
                        Err(e) => {
                            warn!(
                                "[signer][service] failed to decode SignerRequest FB: {:?}",
                                e
                            );
                        }
                    }

                    continue;
                }

                TimeoutFuture::new(sleep_ms).await;
                sleep_ms = (sleep_ms * 2).min(max_sleep);
            }
        });

        info!("[signer][service] service loop spawned");
    }

    fn spawn_nip46_pump(&self) {
        let ws_resp = self.ws_resp.clone();

        spawn_local(async move {
            let mut sleep_ms: u32 = 8;
            let max_sleep: u32 = 256;

            loop {
                let maybe = { ws_resp.borrow_mut().read_next() };

                if let Some(bytes) = maybe {
                    sleep_ms = 8;

                    // Placeholder: In the complete version, decode fb::WorkerMessage,
                    // filter by sub_id, decrypt content (NIP-44/04), correlate by RPC id.
                    // Here we just log a short preview.
                    if let Ok(s) = std::str::from_utf8(&bytes) {
                        let prev = preview(s, 160);
                        info!(
                            "[signer][nip46] ws_response {} bytes: {}",
                            bytes.len(),
                            prev
                        );
                    } else {
                        info!("[signer][nip46] ws_response {} binary bytes", bytes.len());
                    }

                    continue;
                }

                TimeoutFuture::new(sleep_ms).await;
                sleep_ms = (sleep_ms * 2).min(max_sleep);
            }
        });

        info!("[signer][service] NIP-46 pump spawned");
    }

    #[allow(unused)]
    fn publish_frames(&self, relays: &[String], frames: &[String]) {
        // Helper for sending Envelope { relays, frames } to connections via ws_request_signer.
        // connections expects JSON with these two fields (consistent with current Envelope).
        let env = serde_json::json!({ "relays": relays, "frames": frames });
        if let Ok(buf) = serde_json::to_vec(&env) {
            let ok = self.ws_req.borrow_mut().write(&buf);
            if !ok {
                warn!("[signer][service] ws_req ring full, frame dropped");
            }
        }
    }
}

// ----------------------
// Placeholder submodules
// ----------------------

// -------------
// Small helpers
// -------------

fn preview(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let head = &s[..max];
        format!("{}…(+{} bytes)", head, s.len() - max)
    }
}
