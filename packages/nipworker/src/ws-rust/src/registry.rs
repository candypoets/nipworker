use crate::connection::RelayConnection;
use crate::types::{ConnectionStatus, RelayConfig};
use crate::utils::{array_push, create_array};
use js_sys::{Array, Promise};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{future_to_promise, spawn_local, JsFuture};

#[wasm_bindgen]
pub struct ConnectionRegistry {
    connections: HashMap<String, Rc<RefCell<RelayConnection>>>,
    disabled_relays: HashSet<String>,
    next_allowed: HashMap<String, f64>,
    sub_counts: HashMap<String, u32>,
    config: RelayConfig,
    cooldown_ms: u32,
}

#[wasm_bindgen]
impl ConnectionRegistry {
    #[wasm_bindgen(constructor)]
    pub fn new(config: RelayConfig) -> Self {
        Self {
            connections: HashMap::new(),
            disabled_relays: HashSet::new(),
            next_allowed: HashMap::new(),
            sub_counts: HashMap::new(),
            config,
            cooldown_ms: 60000,
        }
    }

    fn now(&self) -> f64 {
        js_sys::Date::now()
    }

    fn detect_kind(&self, frame: &str) -> &'static str {
        if let Some(m) = frame.find(r#"["#) {
            let start = m + 1;
            if let Some(end) = frame[start..].find('"') {
                let k = &frame[start..start + end].to_uppercase();
                if k == "REQ" {
                    return "REQ";
                }
                if k == "CLOSE" {
                    return "CLOSE";
                }
            }
        }
        "OTHER"
    }

    fn get_count(&self, url: &str) -> u32 {
        *self.sub_counts.get(url).unwrap_or(&0)
    }

    fn set_count(&mut self, url: &str, value: u32) {
        self.sub_counts.insert(url.to_string(), value.max(0));
    }

    fn bump_count(&mut self, url: &str, delta: i32) -> u32 {
        let next = (self.get_count(url) as i32 + delta).max(0) as u32;
        self.set_count(url, next);
        next
    }

    fn give_up_or_cooldown(&mut self, url: &str, conn: &Rc<RefCell<RelayConnection>>) {
        if conn.borrow().has_given_up() {
            self.disabled_relays.insert(url.to_string());
            self.next_allowed
                .insert(url.to_string(), self.now() + self.cooldown_ms as f64);
        } else {
            self.next_allowed
                .insert(url.to_string(), self.now() + 10000.0);
        }
    }

    fn is_cooling_down(&self, url: &str) -> bool {
        if let Some(at) = self.next_allowed.get(url) {
            self.now() < *at
        } else {
            false
        }
    }

    pub fn ensure_connection(&mut self, url: &str) -> Promise {
        if self.disabled_relays.contains(url) {
            return Promise::reject(&JsValue::from_str(&format!("Relay disabled: {}", url)));
        }
        if self.is_cooling_down(url) {
            return Promise::reject(&JsValue::from_str(&format!("Relay {} cooling down", url)));
        }

        let conn = if let Some(c) = self.connections.get(url) {
            c.clone()
        } else {
            let mut new_conn = Rc::new(RefCell::new(RelayConnection::new(
                url.to_string(),
                self.config.clone(),
            )));
            new_conn.borrow_mut().connect();
            self.connections.insert(url.to_string(), new_conn.clone());
            new_conn
        };

        let status = conn.borrow().get_status();
        if status != ConnectionStatus::Ready {
            conn.borrow_mut().connect();
            let timeout = self.config.connect_timeout_ms;
            let url_clone = url.to_string();
            let conn_clone = conn.clone();
            future_to_promise(async move {
                JsFuture::from(conn_clone.borrow().wait_for_ready(timeout))
                    .await
                    .unwrap();
                if conn_clone.borrow().get_status() != ConnectionStatus::Ready {
                    // Give up
                    Ok(JsValue::NULL)
                } else {
                    Ok(JsValue::UNDEFINED)
                }
            })
        } else {
            Promise::resolve(&JsValue::UNDEFINED)
        }
    }

    pub fn send_frame(&mut self, url: &str, frame: &str) -> Promise {
        if self.disabled_relays.contains(url) || self.is_cooling_down(url) {
            return Promise::resolve(&JsValue::NULL);
        }

        let kind = self.detect_kind(frame);
        let conn_promise = self.ensure_connection(url);
        let url_clone = url.to_string();
        let frame_clone = frame.to_string();
        let kind_clone = kind.to_string();
        future_to_promise(async move {
            JsFuture::from(conn_promise).await.unwrap();
            // Assume conn is ready; send
            let conn = self.connections.get(&url_clone).unwrap().clone();
            JsFuture::from(conn.borrow_mut().send_message(&frame_clone))
                .await
                .unwrap();
            if kind_clone == "REQ" {
                self.bump_count(&url_clone, 1);
            } else if kind_clone == "CLOSE" {
                let new_count = self.bump_count(&url_clone, -1);
                if new_count == 0 {
                    self.disconnect(&url_clone).await.unwrap();
                }
            }
            Ok(JsValue::UNDEFINED)
        })
    }

    async fn send_all_frames_to_relay(
        &mut self,
        url: &str,
        frames: Vec<String>,
    ) -> Result<(), JsValue> {
        for frame in frames {
            JsFuture::from(self.send_frame(url, &frame)).await?;
        }
        Ok(())
    }

    pub fn send_to_relays(
        &mut self,
        relays_js: JsValue,
        frames_js: JsValue,
        max_successes: Option<u32>,
        max_concurrency: Option<u32>,
    ) -> Promise {
        let relays: Array = relays_js.dyn_into().unwrap();
        let frames: Array = frames_js.dyn_into().unwrap();
        let frames_vec: Vec<String> = (0..frames.length())
            .map(|i| frames.get(i).as_string().unwrap())
            .collect();
        let available: Vec<String> = (0..relays.length())
            .filter_map(|i| {
                let url = relays.get(i).as_string()?;
                if !self.disabled_relays.contains(&url) && !self.is_cooling_down(&url) {
                    Some(url)
                } else {
                    None
                }
            })
            .collect();

        let target = std::cmp::min(max_successes.unwrap_or(5) as usize, available.len());
        if target == 0 {
            return Promise::resolve(&JsValue::UNDEFINED);
        }

        let mut index = 0;
        let mut successes = 0;
        let max_conc = max_concurrency.unwrap_or(5) as usize;

        let promise_array = create_array();
        for url in available.iter().take(target) {
            let frames_clone = frames_vec.clone();
            let url_clone = url.clone();
            let p = future_to_promise(async move {
                self.send_all_frames_to_relay(&url_clone, frames_clone)
                    .await
            });
            array_push(&promise_array, &p);
            successes += 1;
            if successes >= target {
                break;
            }
        }

        // Wait all with JS Promise.all
        let all_p = js_sys::Promise::all(&promise_array);
        future_to_promise(async move {
            JsFuture::from(all_p).await?;
            Ok(JsValue::UNDEFINED)
        })
    }

    pub async fn disconnect(&mut self, url: &str) -> Result<(), JsValue> {
        if let Some(conn) = self.connections.remove(url) {
            JsFuture::from(conn.borrow_mut().close()).await?;
        }
        self.sub_counts.remove(url);
        Ok(())
    }

    pub async fn disconnect_all(&mut self) -> Result<(), JsValue> {
        for url in self.connections.keys().cloned().collect::<Vec<_>>() {
            self.disconnect(&url).await?;
        }
        Ok(())
    }

    pub fn enable_relay(&mut self, url: &str) {
        self.disabled_relays.remove(url);
        self.next_allowed.remove(url);
    }

    pub fn is_relay_disabled(&self, url: &str) -> bool {
        self.disabled_relays.contains(url)
    }

    pub fn get_active_req_count(&self, url: &str) -> u32 {
        self.get_count(url)
    }

    pub fn get_connection_status(&self, url: &str) -> Option<ConnectionStatus> {
        self.connections.get(url).map(|c| c.borrow().get_status())
    }
}
