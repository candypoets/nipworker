use crate::crypto::signers::{nip04, nip44, nip44::ConversationKey};
use crate::generated::nostr::fb;
use crate::types::{Event, Keys, PublicKey, SecretKey};
use crate::utils::extract_first_three;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use tracing::{error, info};

use futures::channel::mpsc;
use futures::StreamExt;

/// Callback for NIP-46 auth challenges: (auth url, request_id).
pub type OnAuthUrl = Rc<RefCell<Option<Rc<dyn Fn(String, String)>>>>;

pub struct Pump;

impl Pump {
    pub fn spawn<F>(
        spawner: F,
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
        auth_pending: Rc<RefCell<HashSet<String>>>,
        on_auth_url: OnAuthUrl,
    ) where
        F: Fn(std::pin::Pin<Box<dyn std::future::Future<Output = ()>>>) + 'static,
    {
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

                let remote_pk = PublicKey::from_hex(pk_to_use).map_err(|e| format!("pk: {}", e))?;
                let keys = Keys::new(SecretKey(secret_bytes));
                let secret = &keys.secret_key;

                if use_nip44 {
                    if let Ok(pt) = nip44::decrypt(
                        cipher,
                        &ConversationKey::derive(secret, &remote_pk)
                            .map_err(|e| format!("nip44 derive: {}", e))?,
                    ) {
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
                auth_pending,
                on_auth_url,
            )
            .await;
        };

        spawner(Box::pin(pump_task));
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
        auth_pending: Rc<RefCell<HashSet<String>>>,
        on_auth_url: OnAuthUrl,
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
                        &auth_pending,
                        &on_auth_url,
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
        auth_pending: &Rc<RefCell<HashSet<String>>>,
        on_auth_url: &OnAuthUrl,
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
                            auth_pending,
                            on_auth_url,
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
                        auth_pending,
                        on_auth_url,
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
        auth_pending: &Rc<RefCell<HashSet<String>>>,
        on_auth_url: &OnAuthUrl,
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
                tag.get(0) == Some(&"p".to_string()) && tag.get(1) == Some(&client_pk.to_string())
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
                        auth_pending,
                        on_auth_url,
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
        auth_pending: &Rc<RefCell<HashSet<String>>>,
        on_auth_url: &OnAuthUrl,
    ) {
        if let Ok(rpc) = serde_json::from_str::<serde_json::Value>(plaintext) {
            let rid = rpc
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();

            // NIP-46 auth challenge: `result` is the literal string "auth_url"
            // and the authorization URL is carried in `error`. This is NOT a
            // final response: surface the URL and keep waiting for the real
            // response, which reuses the same request id. Fires even when the
            // id is not in `pending` (unsolicited challenge during QR
            // discovery).
            if rpc.get("result").and_then(|v| v.as_str()) == Some("auth_url") {
                if let Some(url) = rpc
                    .get("error")
                    .and_then(|v| v.as_str())
                    .filter(|u| !u.is_empty())
                {
                    info!("[nip46] auth challenge received for id={}", rid);
                    auth_pending.borrow_mut().insert(rid.clone());
                    if let Some(cb) = on_auth_url.borrow().as_ref() {
                        cb(url.to_string(), rid.clone());
                    }
                    return;
                }
            }

            let err = rpc
                .get("error")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            // Use as_str() for strings to avoid JSON escaping, otherwise to_string() for objects
            let res = rpc
                .get("result")
                .map(|v| {
                    v.as_str()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| v.to_string())
                })
                .unwrap_or_default();

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

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        pending: Rc<RefCell<HashMap<String, Result<String, String>>>>,
        discovered: Rc<RefCell<Option<String>>>,
        expected_secret: Option<String>,
        on_discovery: Rc<RefCell<Option<Rc<dyn Fn(String)>>>>,
        auth_pending: Rc<RefCell<HashSet<String>>>,
        on_auth_url: OnAuthUrl,
        auth_calls: Rc<RefCell<Vec<(String, String)>>>,
    }

    impl Fixture {
        fn new(expected_secret: Option<String>) -> Self {
            let auth_calls: Rc<RefCell<Vec<(String, String)>>> = Rc::new(RefCell::new(Vec::new()));
            let auth_calls_cb = auth_calls.clone();
            Fixture {
                pending: Rc::new(RefCell::new(HashMap::new())),
                discovered: Rc::new(RefCell::new(None)),
                expected_secret,
                on_discovery: Rc::new(RefCell::new(None)),
                auth_pending: Rc::new(RefCell::new(HashSet::new())),
                on_auth_url: Rc::new(RefCell::new(Some(Rc::new(
                    move |url: String, id: String| {
                        auth_calls_cb.borrow_mut().push((url, id));
                    },
                )))),
                auth_calls,
            }
        }

        fn feed(&self, plaintext: &str) {
            Pump::process_rpc_response(
                plaintext,
                "aa".repeat(32).as_str(),
                &self.pending,
                &self.discovered,
                &self.expected_secret,
                &self.on_discovery,
                &self.auth_pending,
                &self.on_auth_url,
            );
        }
    }

    #[test]
    fn auth_url_challenge_fires_callback_without_resolving_pending() {
        let f = Fixture::new(None);
        f.feed(r#"{"id":"r1","result":"auth_url","error":"https://x/approve"}"#);

        // Callback fired with url + request id, nothing resolved yet.
        assert_eq!(
            f.auth_calls.borrow().as_slice(),
            &[("https://x/approve".to_string(), "r1".to_string())]
        );
        assert!(!f.pending.borrow().contains_key("r1"));
        assert!(f.auth_pending.borrow().contains("r1"));

        // The real response reusing the same id resolves normally afterwards.
        f.feed(r#"{"id":"r1","result":"ack","error":null}"#);
        assert_eq!(
            f.pending.borrow_mut().remove("r1"),
            Some(Ok("ack".to_string()))
        );
    }

    #[test]
    fn auth_url_challenge_unsolicited_qr_discovery_case() {
        // QR discovery: nothing is awaiting the id, expected_secret is set.
        let f = Fixture::new(Some("s3cret".to_string()));
        f.feed(r#"{"id":"disc-1","result":"auth_url","error":"https://x/approve"}"#);

        assert_eq!(
            f.auth_calls.borrow().as_slice(),
            &[("https://x/approve".to_string(), "disc-1".to_string())]
        );
        assert!(!f.pending.borrow().contains_key("disc-1"));
        // Discovery must NOT trigger on the challenge itself.
        assert!(f.discovered.borrow().is_none());

        // The follow-up connect response with the secret triggers discovery
        // and resolves normally.
        f.feed(r#"{"id":"disc-1","result":"s3cret","error":null}"#);
        assert!(f.discovered.borrow().is_some());
        assert_eq!(
            f.pending.borrow_mut().remove("disc-1"),
            Some(Ok("s3cret".to_string()))
        );
    }

    #[test]
    fn auth_url_result_without_url_falls_through_as_error() {
        let f = Fixture::new(None);
        f.feed(r#"{"id":"r2","result":"auth_url","error":null}"#);
        // No usable URL: treated as an ordinary (empty) response, no callback.
        assert!(f.auth_calls.borrow().is_empty());
        assert!(f.pending.borrow().contains_key("r2"));
        assert!(!f.auth_pending.borrow().contains("r2"));
    }
}
