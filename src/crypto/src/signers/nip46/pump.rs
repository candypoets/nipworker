use crate::signers::{nip04, nip44, nip44::ConversationKey};
use gloo_timers::future::TimeoutFuture;
use serde_json::Value;
use shared::generated::nostr::fb;
use shared::types::{Event, Keys, PublicKey, SecretKey};
use shared::utils::extract_first_three;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use tracing::{error, info};
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
                let pk_to_use = if remote_pk_str_for_closure.is_empty() {
                    sender_pk_hex
                } else {
                    &remote_pk_str_for_closure
                };

                let remote_pk = PublicKey::from_hex(pk_to_use)
                    .map_err(|e| format!("pk: {}", e))?;
                let keys = Keys::new(SecretKey(secret_bytes));
                let secret = &keys.secret_key;

                if use_nip44 {
                    if let Ok(pt) = nip44::decrypt(cipher, &ConversationKey::derive(secret, &remote_pk)
                        .map_err(|e| format!("nip44 derive: {}", e))?) {
                        return Ok(pt);
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
        };

        spawn_local(pump_task);
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
            match from_connections_rx.next().await {
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
                None => break,
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
        // Try FlatBuffer-encoded WorkerMessage first
        if let Ok(wm) = flatbuffers::root::<fb::WorkerMessage>(bytes) {
            let sid = wm.sub_id().unwrap_or_default();
            if sid != sub_id {
                return;
            }

            match wm.content_type() {
                fb::Message::Raw => {
                    if let Some(raw_msg) = wm.content_as_raw() {
                        Self::handle_nip46_event(
                            None,
                            Some(raw_msg.raw()),
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
                _ => {}
            }
            return;
        }

        // Fallback for raw JSON
        if let Ok(s) = std::str::from_utf8(bytes) {
            if let Some([first, second, third]) = extract_first_three(s) {
                if matches!(first, Some("\"EVENT\"") | Some("\"event\"")) {
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
            if sub_str.trim_matches('"') != sub_id {
                return;
            }
        }

        if let Ok(event) = Event::from_json(evt_json) {
            if event.kind() != 24133 {
                return;
            }

            let event_pubkey = event.pubkey.to_hex();
            
            // Check if event is addressed to us
            let addressed_to_us = event.tags().iter().any(|tag| {
                tag.get(0) == Some(&"p".to_string())
                    && tag.get(1) == Some(&client_pk.to_string())
            });
            
            if !addressed_to_us {
                return;
            }

            let ciphertext = event.content();
            
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
                    error!("[nip46] Decryption failed: {}", e);
                }
            }
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
            let rid = rpc.get("id").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            let err = rpc.get("error").and_then(|v| v.as_str()).map(|s| s.to_string());
            // Use as_str() for strings to avoid JSON escaping, otherwise to_string() for objects
            let res = rpc.get("result").map(|v| {
                v.as_str().map(|s| s.to_string()).unwrap_or_else(|| v.to_string())
            }).unwrap_or_default();

            let outcome = if let Some(e) = err {
                error!("[nip46] RPC error for id={}: {}", rid, e);
                Err(e)
            } else {
                // Check if this is the connect response (result matches expected_secret)
                if let Some(expected) = expected_secret {
                    if let Some(result_str) = rpc.get("result").and_then(|v| v.as_str()) {
                        if result_str == expected {
                            info!("[nip46] Signer discovered: {}", event_pubkey);
                            *discovered_remote_pubkey.borrow_mut() = Some(event_pubkey.to_string());
                            if let Some(cb) = on_discovery.borrow().as_ref() {
                                cb(event_pubkey.to_string());
                            }
                        }
                    }
                }
                Ok(res)
            };

            pending.borrow_mut().insert(rid, outcome);
        }
    }
}
