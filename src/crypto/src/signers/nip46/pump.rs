use crate::signers::{nip04, nip44, nip44::ConversationKey};
use gloo_timers::future::TimeoutFuture;
use serde_json::Value;
use shared::generated::nostr::fb;
use shared::types::{Event, Keys, PublicKey, SecretKey};
use shared::utils::extract_first_three;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use tracing::{debug, info, warn};
use wasm_bindgen_futures::spawn_local;

use futures_channel::mpsc;
use futures_util::StreamExt;

pub struct Pump;

impl Pump {
    pub fn spawn(
        mut from_connections_rx: mpsc::Receiver<Vec<u8>>,
        sub_id: String,
        remote_pk_str: String,
        pending: Rc<RefCell<HashMap<String, Result<String, String>>>>,
        discovered_remote_pubkey: Rc<RefCell<Option<String>>>,
        client_pk: String,
        expected_secret: Option<String>,
        client_keys: Keys,
        use_nip44: bool,
        on_discovery: Rc<RefCell<Option<Rc<dyn Fn(String)>>>>,
    ) {
        let secret_bytes = client_keys.secret_key.0;

        let pump_task = async move {
            let remote_pk_str_for_closure = remote_pk_str.clone();

            let decrypt_helper = move |cipher: &str,
                                       sender_pk_hex: &str|
                  -> Result<String, String> {
                // In QR mode, we don't know the remote_pk yet, so we use the sender's pubkey from the event
                let pk_to_use = if remote_pk_str_for_closure.is_empty() {
                    sender_pk_hex
                } else {
                    &remote_pk_str_for_closure
                };

                let remote_pk = PublicKey::from_hex(pk_to_use).map_err(|e| format!("pk: {}", e))?;
                let keys = Keys::new(SecretKey(secret_bytes));
                let secret = &keys.secret_key;

                if use_nip44 {
                    let conv = ConversationKey::derive(secret, &remote_pk)
                        .map_err(|e| format!("nip44 derive: {}", e))?;
                    match nip44::decrypt(cipher, &conv) {
                        Ok(pt) => return Ok(pt),
                        Err(e) => {
                            debug!("[nip46] pump nip44 decrypt failed, trying nip04: {}", e);
                        }
                    }
                }
                nip04::decrypt(secret, &remote_pk, cipher)
                    .map_err(|e| format!("nip04 decrypt: {}", e))
            };

            Self::run_pump_loop(
                &mut from_connections_rx,
                sub_id,
                pending,
                discovered_remote_pubkey,
                client_pk,
                expected_secret,
                decrypt_helper,
                on_discovery,
            )
            .await;

            info!("[nip46] response pump ended");
        };

        spawn_local(pump_task);
        info!("[nip46] response pump started");
    }

    async fn run_pump_loop(
        from_connections_rx: &mut mpsc::Receiver<Vec<u8>>,
        sub_id: String,
        pending: Rc<RefCell<HashMap<String, Result<String, String>>>>,
        discovered_remote_pubkey: Rc<RefCell<Option<String>>>,
        client_pk: String,
        expected_secret: Option<String>,
        decrypt_helper: impl Fn(&str, &str) -> Result<String, String>,
        on_discovery: Rc<RefCell<Option<Rc<dyn Fn(String)>>>>,
    ) {
        loop {
            // Use .next().await instead of polling with timeout
            let bytes_opt = from_connections_rx.next().await;

            match bytes_opt {
                Some(bytes) => {
                    Self::handle_nip46_frame(
                        &bytes,
                        &sub_id,
                        &pending,
                        &discovered_remote_pubkey,
                        &client_pk,
                        &expected_secret,
                        &decrypt_helper,
                        &on_discovery,
                    )
                    .await;
                }
                None => {
                    info!("[nip46] from_connections channel closed, exiting pump loop");
                    break;
                }
            }
        }
    }

    async fn handle_nip46_frame(
        bytes: &[u8],
        sub_id: &str,
        pending: &Rc<RefCell<HashMap<String, Result<String, String>>>>,
        discovered_remote_pubkey: &Rc<RefCell<Option<String>>>,
        client_pk: &str,
        expected_secret: &Option<String>,
        decrypt_helper: &impl Fn(&str, &str) -> Result<String, String>,
        on_discovery: &Rc<RefCell<Option<Rc<dyn Fn(String)>>>>,
    ) {
        // The connections worker sends FlatBuffer-encoded WorkerMessage
        if let Ok(wm) = flatbuffers::root::<fb::WorkerMessage>(bytes) {
            let sid = wm.sub_id().unwrap_or_default();
            if sid != sub_id {
                debug!(
                    "[nip46] FlatBuffer sub_id mismatch: expected={}, got={}",
                    sub_id, sid
                );
                return;
            }

            match wm.content_type() {
                fb::Message::Raw => {
                    if let Some(raw_msg) = wm.content_as_raw() {
                        let raw_json = raw_msg.raw();
                        info!(
                            "[nip46] Received Raw FlatBuffer message for sub_id: {}",
                            sid
                        );
                        // The raw_json here is the content of the EVENT (the event object itself)
                        // or the full frame if parsing failed.
                        // However, handle_nip46_event expects the second and third parts of the NIP-01 array.
                        // Since build_worker_message for EVENT puts the event object in the Raw field,
                        // we can reconstruct the parts.
                        Self::handle_nip46_event(
                            None, // sub_id already verified above
                            Some(raw_json),
                            sub_id,
                            pending,
                            discovered_remote_pubkey,
                            client_pk,
                            expected_secret,
                            decrypt_helper,
                            on_discovery,
                        )
                        .await;
                    }
                }
                fb::Message::ConnectionStatus => {
                    if let Some(cs) = wm.content_as_connection_status() {
                        info!(
                            "[nip46] Received ConnectionStatus: status={:?}, message={:?}",
                            cs.status(),
                            cs.message()
                        );
                    }
                }
                _ => {
                    info!(
                        "[nip46] Received non-Raw FlatBuffer message type: {:?}",
                        wm.content_type()
                    );
                }
            }
            return;
        }

        // Fallback for raw JSON (if any)
        match std::str::from_utf8(bytes) {
            Ok(s) => match extract_first_three(s) {
                Some([first, second, third]) => match first {
                    Some("\"EVENT\"") | Some("\"event\"") => {
                        Self::handle_nip46_event(
                            second,
                            third,
                            sub_id,
                            pending,
                            discovered_remote_pubkey,
                            client_pk,
                            expected_secret,
                            decrypt_helper,
                            on_discovery,
                        )
                        .await;
                    }
                    _ => {}
                },
                None => {
                    warn!("[nip46] Failed to parse JSON array frame");
                }
            },
            Err(_) => {
                debug!("[nip46] ws_response {} binary bytes", bytes.len());
            }
        }
    }

    async fn handle_nip46_event(
        second: Option<&str>,
        third: Option<&str>,
        sub_id: &str,
        pending: &Rc<RefCell<HashMap<String, Result<String, String>>>>,
        discovered_remote_pubkey: &Rc<RefCell<Option<String>>>,
        client_pk: &str,
        expected_secret: &Option<String>,
        decrypt_helper: &impl Fn(&str, &str) -> Result<String, String>,
        on_discovery: &Rc<RefCell<Option<Rc<dyn Fn(String)>>>>,
    ) {
        let (maybe_sub, evt_json) = match (second, third) {
            (Some(sub), Some(evt)) => (Some(sub), evt),
            (None, Some(evt)) => (None, evt),
            _ => return,
        };

        if let Some(sub_str) = maybe_sub {
            let sub_id_clean = sub_str.trim_matches('"');
            if sub_id_clean != sub_id {
                return;
            }
        }

        if let Ok(event) = Event::from_json(evt_json) {
            if event.kind() != 24133 {
                debug!("[nip46] Ignoring event kind: {}", event.kind());
                return;
            }

            let event_pubkey = event.pubkey.to_hex();
            let mut addressed_to_us = false;
            for tag in event.tags() {
                if tag.get(0) == Some(&"p".to_string())
                    && tag.get(1) == Some(&client_pk.to_string())
                {
                    addressed_to_us = true;
                    break;
                }
            }
            if !addressed_to_us {
                debug!("[nip46] Event not addressed to us (p-tag mismatch)");
                return;
            }

            let ciphertext = event.content();
            info!("[nip46] Processing event from {}", event_pubkey);
            match decrypt_helper(ciphertext, &event_pubkey) {
                Ok(pt) => {
                    Self::process_rpc_response(
                        &pt,
                        &event_pubkey,
                        pending,
                        discovered_remote_pubkey,
                        expected_secret,
                        on_discovery,
                    );
                }
                Err(e) => {
                    warn!("[nip46] Decryption failed: {}", e);
                }
            }
        } else {
            warn!("[nip46] Failed to parse event JSON: {}", evt_json);
        }
    }

    fn process_rpc_response(
        plaintext: &str,
        event_pubkey: &str,
        pending: &Rc<RefCell<HashMap<String, Result<String, String>>>>,
        discovered_remote_pubkey: &Rc<RefCell<Option<String>>>,
        expected_secret: &Option<String>,
        on_discovery: &Rc<RefCell<Option<Rc<dyn Fn(String)>>>>,
    ) {
        if let Ok(rpc) = serde_json::from_str::<Value>(plaintext) {
            let rid = rpc
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();

            let err = rpc
                .get("error")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let res = rpc
                .get("result")
                .and_then(|v| {
                    if v.is_string() {
                        v.as_str().map(|s| s.to_string())
                    } else {
                        Some(v.to_string())
                    }
                })
                .unwrap_or_default();
            info!(
                "[nip46] RPC response processed: id={} result={} error={:?}",
                rid, res, err
            );
            let outcome = if let Some(e) = err {
                Err(e)
            } else {
                if let Some(expected_secret) = expected_secret {
                    if let Some(result) = rpc.get("result") {
                        if let Some(result_str) = result.as_str() {
                            if result_str == expected_secret {
                                let remote_pubkey = event_pubkey;
                                info!("[nip46] Remote signer discovered: {}", remote_pubkey);
                                *discovered_remote_pubkey.borrow_mut() =
                                    Some(remote_pubkey.to_string());
                                if let Some(cb) = on_discovery.borrow().as_ref() {
                                    cb(remote_pubkey.to_string());
                                }
                                Ok(res)
                            } else {
                                Err("Invalid secret in response".to_string())
                            }
                        } else {
                            Err("Invalid result format".to_string())
                        }
                    } else {
                        Err("No result in response".to_string())
                    }
                } else {
                    Ok(res)
                }
            };

            pending.borrow_mut().insert(rid, outcome);
        }
    }
}
