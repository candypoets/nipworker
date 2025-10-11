use crate::types::{ConnectionStatus, MessageHandler, RelayConfig, RelayStats};
use crate::ws_interop::{create_websocket_js, ws_close_js, ws_send_js};
use js_sys::{Date, Promise};
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{future_to_promise, spawn_local, JsFuture};

#[wasm_bindgen]
pub struct RelayConnection {
    url: String,
    config: RelayConfig,
    status: Rc<RefCell<ConnectionStatus>>,
    ws: Rc<RefCell<JsValue>>, // WS object from JS
    want_reconnect: Rc<RefCell<bool>>,
    attempts: Rc<RefCell<u32>>,
    given_up: Rc<RefCell<bool>>,
    last_activity: Rc<RefCell<f64>>,
    stats: Rc<RefCell<RelayStats>>,
    ready_waiters: Vec<Closure<dyn FnMut(bool)>>,
    message_handler: Option<MessageHandler>,
    reconnect_timer: Option<JsFuture>,
}

#[wasm_bindgen]
impl RelayConnection {
    #[wasm_bindgen(constructor)]
    pub fn new(url: String, config: RelayConfig) -> Self {
        let status = Rc::new(RefCell::new(ConnectionStatus::Closed));
        let ws = Rc::new(RefCell::new(JsValue::NULL));
        let want_reconnect = Rc::new(RefCell::new(true));
        let attempts = Rc::new(RefCell::new(0u32));
        let given_up = Rc::new(RefCell::new(false));
        let last_activity = Rc::new(RefCell::new(Date::now()));
        let stats = Rc::new(RefCell::new(RelayStats {
            sent: 0,
            received: 0,
            reconnects: 0,
            last_activity: Date::now(),
            dropped: 0,
        }));
        Self {
            url,
            config,
            status: status.clone(),
            ws: ws.clone(),
            want_reconnect: want_reconnect.clone(),
            attempts: attempts.clone(),
            given_up: given_up.clone(),
            last_activity: last_activity.clone(),
            stats: stats.clone(),
            ready_waiters: Vec::new(),
            message_handler: None,
            reconnect_timer: None,
        }
    }

    pub fn get_url(&self) -> String {
        self.url.clone()
    }

    pub fn get_status(&self) -> ConnectionStatus {
        *self.status.borrow()
    }

    pub fn get_stats(&self) -> RelayStats {
        let s = self.stats.borrow().clone();
        RelayStats {
            reconnects: *self.attempts.borrow(),
            last_activity: *self.last_activity.borrow(),
            ..s
        }
    }

    pub fn get_last_activity(&self) -> f64 {
        *self.last_activity.borrow()
    }

    pub fn has_given_up(&self) -> bool {
        *self.given_up.borrow()
    }

    pub fn set_message_handler(&mut self, handler: MessageHandler) {
        self.message_handler = Some(handler);
    }

    pub fn connect(&mut self) -> JsValue {
        if *self.given_up.borrow() {
            return JsValue::from_bool(false);
        }
        if *self.status.borrow() == ConnectionStatus::Connecting
            || *self.status.borrow() == ConnectionStatus::Ready
        {
            return JsValue::from_bool(true);
        }

        *self.status.borrow_mut() = ConnectionStatus::Connecting;
        let ws = create_websocket_js(&self.url);
        *self.ws.borrow_mut() = ws.clone();

        // Setup JS event handlers (via global JS; assume JS sets onopen/onclose etc. to call Rust methods)
        // For simplicity, spawn async for timeout
        let config_timeout = self.config.connect_timeout_ms;
        let status_clone = self.status.clone();
        spawn_local(async move {
            JsFuture::from(sleep_ms(config_timeout)).await.unwrap();
            if *status_clone.borrow() == ConnectionStatus::Connecting {
                *status_clone.borrow_mut() = ConnectionStatus::Closed;
                // Resolve waiters false
            }
        });

        // Assume JS handles onopen/onclose to update status via exposed methods (e.g., rust_update_status)
        JsValue::from_bool(true) // Fire-and-forget
    }

    pub fn send_message(&mut self, frame: &str) -> Promise {
        if *self.status.borrow() != ConnectionStatus::Ready {
            return Promise::reject(&JsValue::from_str("Connection not ready"));
        }
        *self.stats.borrow_mut().sent += 1;
        *self.last_activity.borrow_mut() = Date::now();
        let ws = self.ws.borrow().clone();
        ws_send_js(&ws, frame)
    }

    pub fn wait_for_ready(&self, timeout_ms: u32) -> Promise {
        if *self.status.borrow() == ConnectionStatus::Ready {
            return Promise::resolve(&JsValue::UNDEFINED);
        }

        future_to_promise(async move {
            JsFuture::from(sleep_ms(timeout_ms)).await.unwrap();
            if *self.status.borrow() == ConnectionStatus::Ready {
                Ok(JsValue::UNDEFINED)
            } else {
                Err(JsValue::from_str("Timeout or closed"))
            }
        })
    }

    pub fn close(&mut self) -> Promise {
        *self.want_reconnect.borrow_mut() = false;
        if let Some(timer) = &self.reconnect_timer {
            // Clear timer (assume JS clearTimeout)
        }
        let ws = self.ws.borrow().clone();
        *self.status.borrow_mut() = ConnectionStatus::Closed;
        ws_close_js(&ws)
    }

    pub fn should_close_due_to_inactivity(&self) -> bool {
        Date::now() - *self.last_activity.borrow() > self.config.idle_timeout_ms as f64
    }

    // Internal: Called by JS on events
    #[wasm_bindgen]
    pub fn update_status(&mut self, new_status: ConnectionStatus, ok: bool) {
        *self.status.borrow_mut() = new_status;
        if new_status == ConnectionStatus::Ready {
            *self.attempts.borrow_mut() = 0;
            *self.given_up.borrow_mut() = false;
            *self.last_activity.borrow_mut() = Date::now();
        }
        // Resolve waiters
        self.ready_waiters.clear(); // Simplified; in full, call closures
        if !ok {
            self.schedule_reconnect();
        }
    }

    fn schedule_reconnect(&mut self) {
        if !*self.want_reconnect.borrow() {
            return;
        }
        if *self.status.borrow() != ConnectionStatus::Closed {
            return;
        }

        let cap = self.config.max_reconnect_attempts;
        if cap > 0 && *self.attempts.borrow() >= cap {
            *self.given_up.borrow_mut() = true;
            return;
        }

        let base = self.config.retry.base_ms;
        let max = self.config.retry.max_ms;
        let mult = self.config.retry.multiplier;
        let jitter = self.config.retry.jitter;
        let attempts = *self.attempts.borrow();
        let delay = (base as f64 * mult.powf(attempts as f64)).min(max as f64)
            * (1.0 + (js_sys::Math::random() - 0.5) * jitter * 2.0) as u32;

        let mut_clone = self.clone();
        self.reconnect_timer = Some(JsFuture::from(sleep_ms(delay)));
        spawn_local(async move {
            JsFuture::from(mut_clone.reconnect_timer.unwrap())
                .await
                .unwrap();
            *mut_clone.attempts.borrow_mut() += 1;
            *mut_clone.stats.borrow_mut().reconnects = *mut_clone.attempts.borrow();
            mut_clone.connect();
        });
    }

    fn extract_sub_id(&self, s: &str) -> Option<String> {
        let mut i = 0;
        let n = s.len();
        while i < n && s.as_bytes()[i] as char <= ' ' {
            i += 1;
        }
        if i >= n || s.as_bytes()[i] as char != '[' as char {
            return None;
        }
        i += 1;
        while i < n && s.as_bytes()[i] as char <= ' ' {
            i += 1;
        }
        if i >= n {
            return None;
        }
        if s.as_bytes()[i] as char == '"' as char {
            i += 1;
            while i < n && s.as_bytes()[i] as char != '"' as char {
                i += 1;
            }
            if i >= n {
                return None;
            }
            i += 1;
        } else {
            while i < n
                && s.as_bytes()[i] as char != ',' as char
                && s.as_bytes()[i] as char != ']' as char
            {
                i += 1;
            }
        }
        while i < n && s.as_bytes()[i] as char != ',' as char {
            i += 1;
        }
        if i >= n || s.as_bytes()[i] as char != ',' as char {
            return None;
        }
        i += 1;
        while i < n && s.as_bytes()[i] as char <= ' ' {
            i += 1;
        }
        if i >= n {
            return None;
        }
        if s.as_bytes()[i] as char == '"' as char {
            i += 1;
            let start = i;
            while i < n && s.as_bytes()[i] as char != '"' as char {
                i += 1;
            }
            if i > n {
                return None;
            }
            Some(s[start..i].to_string())
        } else {
            None
        }
    }
}
