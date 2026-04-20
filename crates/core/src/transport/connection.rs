//! Individual Relay Connection Management
//!
//! Revised design goals:
//! - Connect immediately on construction.
//! - Enqueue frames immediately; only start draining once the connection is established.
//! - If not connected while draining, attempt reconnection, then retry the same frame once.
//! - If reconnect/retry fails, drop that frame, mark relay as unreliable for a cooldown window,
//!   and avoid further reconnect attempts during that window.
//! - Synthetic notifications are emitted on successful send: REQ => SUBSCRIBED, CLOSE => CLOSED.
//!
//! Notes:
//! - Synchronous, non-blocking enqueue via bounded channel (cap: 50). `send_raw` never awaits network.
//! - Incoming messages are written to ring buffer via `out_writer`. Status changes via `status_writer`.

use crate::platform::{now_millis, sleep};
use crate::spawn::spawn_worker;
use crate::traits::{RelayTransport, TransportStatus};
use crate::transport::types::{
	AuthState, ConnectionStats, ConnectionStatus, RelayError,
};
use crate::utils::{extract_first_three, validate_relay_url};

use futures::channel::mpsc::{self, Receiver, Sender};
use futures::StreamExt;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::{Arc, RwLock};
use tracing::warn;

type OutWriter = Rc<dyn Fn(&str, &str, &str)>; // (url, sub_id, raw_text)
type StatusWriter = Rc<dyn Fn(&str, &str)>; // (status, url)
type CryptoSender = Rc<RefCell<dyn Fn(&[u8])>>;

const RECONNECT_RETRY_DELAY_MS: u64 = 30_000;
const HEALTHY_RETRY_DELAYS_MS: [u32; 3] = [200, 800, 2_000];
const COLD_START_RETRY_DELAYS_MS: [u32; 1] = [200];

fn normalize_token(token: &str) -> String {
	let t = token.trim();
	let b = t.as_bytes();
	if b.len() >= 2 && b[0] == b'"' && b[b.len() - 1] == b'"' {
		t[1..b.len() - 1].to_string()
	} else {
		t.to_string()
	}
}

/// Parse incoming relay frame and return (kind, sub_id, content_for_auth)
fn parse_incoming_relay_text(text: &str) -> Option<(String, Option<String>, Option<String>)> {
	// Strict JSON parsing first
	if let Ok(Value::Array(arr)) = serde_json::from_str::<Value>(text) {
		let kind = arr.get(0)?.as_str()?.to_string();

		let sub_id = match kind.as_str() {
			"EVENT" | "EOSE" | "OK" | "CLOSED" => {
				arr.get(1).and_then(|v| v.as_str()).map(ToString::to_string)
			}
			_ => None,
		};

		let content = match kind.as_str() {
			"AUTH" | "NOTICE" => arr.get(1).map(|v| {
				v.as_str()
					.map(ToString::to_string)
					.unwrap_or_else(|| v.to_string())
			}),
			"OK" => {
				// Keep both accepted flag + message for tolerant downstream checks.
				let mut parts = Vec::new();
				if let Some(v) = arr.get(2) {
					parts.push(v.to_string());
				}
				if let Some(v) = arr.get(3) {
					parts.push(v.to_string());
				}
				if parts.is_empty() {
					None
				} else {
					Some(parts.join(","))
				}
			}
			_ => arr.get(2).map(|v| v.to_string()),
		};

		return Some((kind, sub_id, content));
	}

	// Tolerant fallback
	let [k_opt, second_opt, third_opt] = extract_first_three(text)?;
	let kind = normalize_token(k_opt?).to_string();

	let (sub_id, content) = match kind.as_str() {
		"AUTH" | "NOTICE" => (
			None,
			second_opt
				.map(normalize_token)
				.or_else(|| third_opt.map(normalize_token)),
		),
		"EVENT" | "EOSE" | "OK" | "CLOSED" => (
			second_opt.map(normalize_token),
			third_opt.map(normalize_token),
		),
		_ => (
			second_opt.map(normalize_token),
			third_opt.map(normalize_token),
		),
	};

	Some((kind, sub_id, content))
}

pub struct RelayConnection {
	url: String,
	status: Arc<RwLock<ConnectionStatus>>,
	transport: Arc<dyn RelayTransport>,

	stats: Arc<RwLock<ConnectionStats>>,
	active_subs: Arc<RwLock<HashSet<String>>>,
	backoff_attempts: Arc<RwLock<u32>>,
	next_retry_at_ms: Arc<RwLock<u64>>,

	// Channel created at construction time so callers can enqueue immediately.
	queue_tx: Arc<RwLock<Option<Sender<String>>>>,
	// Receiver is held until first successful connect, then consumed by the drainer.
	queue_rx: Arc<RwLock<Option<Receiver<String>>>>,

	out_writer: OutWriter,
	status_writer: StatusWriter,

	// NIP-42 authentication state
	auth_state: Arc<RwLock<AuthState>>,
	// Frames queued before auth state is known (Unknown) or during auth handshake (Required)
	pre_auth_queue: Arc<RwLock<Vec<String>>>,
	// Callback to send signing requests to crypto worker
	to_crypto: CryptoSender,
	// Counter for generating unique auth request IDs
	next_auth_id: Arc<RwLock<u64>>,
}

impl RelayConnection {
	pub fn new(
		url: String,
		transport: Arc<dyn RelayTransport>,
		out_writer: OutWriter,
		status_writer: StatusWriter,
		to_crypto: CryptoSender,
	) -> Arc<Self> {
		// Create the queue immediately so send_raw can enqueue even before connection.
		let (tx, rx) = mpsc::channel::<String>(64);

		let conn = Arc::new(Self {
			url,
			status: Arc::new(RwLock::new(ConnectionStatus::Connecting)),
			transport,
			stats: Arc::new(RwLock::new(ConnectionStats::default())),
			active_subs: Arc::new(RwLock::new(HashSet::new())),
			backoff_attempts: Arc::new(RwLock::new(0)),
			next_retry_at_ms: Arc::new(RwLock::new(0)),
			queue_tx: Arc::new(RwLock::new(Some(tx))),
			queue_rx: Arc::new(RwLock::new(Some(rx))),
			out_writer,
			status_writer,
			auth_state: Arc::new(RwLock::new(AuthState::Unknown)),
			pre_auth_queue: Arc::new(RwLock::new(Vec::new())),
			to_crypto,
			next_auth_id: Arc::new(RwLock::new(1)),
		});

		// Connect immediately
		(conn.status_writer)("connecting", &conn.url);
		let this = Arc::clone(&conn);
		spawn_worker(async move {
			if let Err(e) = this.connect().await {
				tracing::error!(relay = %this.url, error = ?e, "Initial connect failed");
				(this.status_writer)("failed", &this.url);
			}
		});

		conn
	}

	pub fn url(&self) -> &str {
		&self.url
	}

	#[inline]
	fn clear_backoff(&self) {
		*self.backoff_attempts.write().unwrap() = 0;
		*self.next_retry_at_ms.write().unwrap() = 0;
	}

	#[inline]
	fn should_delay_reconnect(&self) -> bool {
		let now = now_millis();
		let retry_at = *self.next_retry_at_ms.read().unwrap();
		now < retry_at
	}

	#[inline]
	fn set_next_retry_delay_ms(&self, delay_ms: u64) {
		let retry_at = now_millis() + delay_ms;
		*self.next_retry_at_ms.write().unwrap() = retry_at;
	}

	// Initialize queue drainer once, after a successful connect: take the receiver and spawn drainer.
	fn init_queue_drainer(self: &Arc<Self>) {
		// Only start the drainer if we still have the receiver (i.e., not started yet).
		let maybe_rx = { self.queue_rx.write().unwrap().take() };
		if let Some(rx) = maybe_rx {
			let conn = Arc::clone(self);
			spawn_worker(async move {
				conn.queue_drainer(rx).await;
			});
		}
	}

	// Drainer owns Arc<Self> to keep the connection alive while draining.
	// Policy:
	// - Try immediate send first.
	// - If send fails, attempt one immediate reconnect + one retry for the same frame.
	// - If reconnect/retry fails, perform staged forced reconnect+retry attempts (mobile timing mitigation).
	// - If relay was previously healthy (Connected), try harder before cooldown.
	// - If all retries fail, drop that frame, mark relay unreliable (cooldown window), continue.
	// - While unreliable window is active, skip reconnect attempts and drop incoming queued frames quickly.
	async fn queue_drainer(self: Arc<Self>, mut rx: Receiver<String>) {
		while let Some(frame) = rx.next().await {
			let now = now_millis();
			let retry_at = *self.next_retry_at_ms.read().unwrap();

			if now < retry_at {
				let remaining = (retry_at - now) / 1000;
				tracing::warn!(
					relay = %self.url,
					remaining_secs = remaining,
					frame_len = frame.len(),
					"[connections][drainer] relay unreliable during cooldown; dropping queued frame without reconnect"
				);
				continue;
			}

			let was_previously_connected =
				matches!(*self.status.read().unwrap(), ConnectionStatus::Connected);

			tracing::info!(
				relay = %self.url,
				status = ?*self.status.read().unwrap(),
				was_previously_connected,
				frame_len = frame.len(),
				"[connections][drainer] processing queued frame"
			);

			// Attempt direct send first (covers the common connected case).
			match self.send_raw_internal(&frame).await {
				Ok(()) => {
					tracing::info!(relay = %self.url, "[connections][drainer] frame sent successfully (direct)");
					continue;
				}
				Err(e) => {
					tracing::warn!(
						relay = %self.url,
						error = ?e,
						"[connections][drainer] direct send failed; attempting immediate reconnect + single retry"
					);
				}
			}

			// Reconnect attempt #1 for this frame.
			match self.connect().await {
				Ok(()) => {
					tracing::info!(relay = %self.url, "[connections][drainer] reconnect #1 succeeded; retrying same frame");
					match self.send_raw_internal(&frame).await {
						Ok(()) => {
							tracing::info!(relay = %self.url, "[connections][drainer] frame sent successfully after reconnect #1 retry");
							continue;
						}
						Err(e) => {
							tracing::warn!(
								relay = %self.url,
								error = ?e,
								"[connections][drainer] retry send failed after reconnect #1; trying one forced reconnect retry"
							);
						}
					}
				}
				Err(e) => {
					tracing::warn!(
						relay = %self.url,
						error = ?e,
						"[connections][drainer] reconnect #1 failed; trying one forced reconnect retry"
					);
				}
			}

			let delays: &[u32] = if was_previously_connected {
				&HEALTHY_RETRY_DELAYS_MS
			} else {
				&COLD_START_RETRY_DELAYS_MS
			};

			let mut delivered = false;
			for (idx, delay_ms) in delays.iter().copied().enumerate() {
				// If connect() set cooldown on previous failure path, clear it for immediate staged attempts.
				self.clear_backoff();
				sleep(delay_ms as u64).await;

				let attempt_num = idx + 2; // #1 already happened above
				match self.connect().await {
					Ok(()) => {
						tracing::info!(
							relay = %self.url,
							attempt_num,
							delay_ms,
							"[connections][drainer] reconnect staged attempt succeeded; retrying same frame"
						);
						match self.send_raw_internal(&frame).await {
							Ok(()) => {
								tracing::info!(
									relay = %self.url,
									attempt_num,
									"[connections][drainer] frame sent successfully after staged reconnect retry"
								);
								delivered = true;
								break;
							}
							Err(e) => {
								tracing::warn!(
									relay = %self.url,
									attempt_num,
									error = ?e,
									"[connections][drainer] staged retry send failed"
								);
							}
						}
					}
					Err(e) => {
						tracing::warn!(
							relay = %self.url,
							attempt_num,
							error = ?e,
							"[connections][drainer] staged reconnect failed"
						);
					}
				}
			}

			if delivered {
				continue;
			}

			tracing::error!(
				relay = %self.url,
				was_previously_connected,
				"[connections][drainer] all reconnect/send retries exhausted; dropping frame and marking unreliable"
			);
			self.set_next_retry_delay_ms(RECONNECT_RETRY_DELAY_MS);
			let retry_at = *self.next_retry_at_ms.read().unwrap();
			tracing::warn!(relay = %self.url, retry_at, "[connections][drainer] relay marked unreliable after retry exhaustion");
		}
		tracing::debug!(relay = %self.url, "Queue drainer exiting");
	}

	async fn send_raw_internal(&self, text: &str) -> Result<(), RelayError> {
		self.transport
			.send(&self.url, text.to_string())
			.await
			.map_err(|e| RelayError::ConnectionError(e.to_string()))?;

		// On successful send, adjust inflight and emit synthetic notifications when appropriate.
		if let Some(parts) = extract_first_three(text) {
			if let Some(kind_raw) = parts[0] {
				let k = kind_raw.trim_matches('"');
				match k {
					"CLOSE" => {
						if let Some(sub) = parts[1].map(|s| s.trim_matches('"').to_string()) {
							// New: use membership instead of a counter
							{
								let mut set = self.active_subs.write().unwrap();
								set.remove(&sub);
								if set.is_empty() {
									// Preserve the existing "auto-close when no subs remain"
									let _ = self.close();
								}
							}
							let raw_closed = format!(r#"["OK","{}","CLOSED"]"#, sub);
							(self.out_writer)(&self.url, &sub, &raw_closed);
						}
					}
					"REQ" => {
						if let Some(sub_id) = parts[1].map(|s| s.trim_matches('"').to_string()) {
							self.active_subs.write().unwrap().insert(sub_id.clone());
							// optional: keep the synthetic notification
							let raw_subscribed = format!(r#"["OK","{}","SUBSCRIBED"]"#, sub_id);
							(self.out_writer)(&self.url, &sub_id, &raw_subscribed);
						}
					}
					"EVENT" => {
						// parts[1] is the event JSON object; extract "id":"<hex>"
						if let Some(event_obj) = parts[1] {
							// Minimal, allocation-light extraction of the id field
							let mut event_id: Option<String> = None;
							if let Some(pos) = event_obj.find("\"id\"") {
								if let Some(colon) = event_obj[pos..].find(':') {
									let rest = &event_obj[pos + colon + 1..];
									let rest = rest.trim_start();
									if rest.starts_with('"') {
										if let Some(endq) = rest[1..].find('"') {
											let id = &rest[1..1 + endq];
											if !id.is_empty() {
												event_id = Some(id.to_string());
											}
										}
									}
								}
							}

							if let Some(id) = event_id {
								// Synthetic OK to indicate the publish has been sent
								// This routes by event_id (used as sub_id for publish tracking)
								let raw_sent = format!(r#"["OK","{}","SENT"]"#, id);
								(self.out_writer)(&self.url, &id, &raw_sent);
							}
						}
					}
					_ => {}
				}
			}
		}

		Ok(())
	}

	// Public API: enqueue a frame (sync, non-blocking).
	// No readiness logic here; the drainer enforces connection state and reconnection.
	// Important: we always enqueue (unless queue full), even if auth is currently Failed,
	// so stale transport state does not cause immediate caller-side frame loss.
	pub fn send_raw(self: &Arc<Self>, text: &str) -> Result<(), RelayError> {
		if let Some(tx) = self.queue_tx.read().unwrap().as_ref() {
			tracing::info!(
				relay = %self.url,
				frame_len = text.len(),
				auth_state = ?*self.auth_state.read().unwrap(),
				status = ?*self.status.read().unwrap(),
				"[connections][enqueue] enqueueing frame"
			);
			tx.clone().try_send(text.to_owned()).map_err(|e| {
				if e.is_full() {
					warn!(relay = %self.url, "Frame dropped: send queue full (64)");
					RelayError::QueueFull
				} else {
					tracing::error!(relay = %self.url, "[connections][enqueue] queue closed while enqueueing");
					RelayError::ConnectionClosed
				}
			})
		} else {
			tracing::error!(relay = %self.url, "No queue sender available");
			Err(RelayError::ConnectionClosed)
		}
	}

	async fn connect(self: &Arc<Self>) -> Result<(), RelayError> {
		// Respect retry timestamp gate for any (re)connect attempt.
		if self.should_delay_reconnect() {
			let now = now_millis();
			let retry_at = *self.next_retry_at_ms.read().unwrap();
			let remaining = retry_at.saturating_sub(now) / 1000;
			tracing::warn!(relay = %self.url, remaining_secs = remaining, "[connections][connect] reconnect blocked by unreliable cooldown");
			return Err(RelayError::ConnectionClosed);
		}

		tracing::info!(relay = %self.url, status = ?*self.status.read().unwrap(), "[connections][connect] opening websocket");

		// Mark connecting now so old transport termination is treated as intentional replacement.
		{
			let mut st = self.status.write().unwrap();
			*st = ConnectionStatus::Connecting;
		}
		(self.status_writer)("connecting", &self.url);

		validate_relay_url(&self.url).map_err(|e| RelayError::InvalidUrl(e.to_string()))?;

		// Open transport connection
		self.transport.connect(&self.url).await.map_err(|e| {
			let mut st = self.status.write().unwrap();
			*st = ConnectionStatus::Failed;
			(self.status_writer)("failed", &self.url);
			// Retry delay to avoid hammering failed relays.
			self.set_next_retry_delay_ms(RECONNECT_RETRY_DELAY_MS);
			RelayError::ConnectionError(e.to_string())
		})?;

		// Register incoming message callback
		let conn = Arc::clone(self);
		self.transport.on_message(&self.url, Box::new(move |text: String| {
			conn.handle_incoming_message(&text);
		}));

		// Register transport status callback
		let conn = Arc::clone(self);
		self.transport.on_status(&self.url, Box::new(move |status| {
			conn.handle_transport_status(status);
		}));

		// Mark connected
		{
			let mut st = self.status.write().unwrap();
			*st = ConnectionStatus::Connected;
		}
		{
			let mut s = self.stats.write().unwrap();
			s.connected_at = Some(now_millis());
		}

		// Reset auth flow on each fresh transport connect.
		// First incoming relay frame determines if auth is needed.
		{
			let old = std::mem::replace(&mut *self.auth_state.write().unwrap(), AuthState::Unknown);
			tracing::info!(relay = %self.url, old_auth_state = ?old, "[connections][connect] auth state reset to Unknown after connect");
		}

		(self.status_writer)("connected", &self.url);
		self.clear_backoff();
		tracing::info!(relay = %self.url, "[connections][connect] websocket connected");

		// Start the queue drainer on first successful connection
		self.init_queue_drainer();

		Ok(())
	}

	fn handle_incoming_message(&self, text: &str) {
		tracing::info!(relay = %self.url, "Raw incoming: {}", text);

		if let Some((kind, sub_id, content)) = parse_incoming_relay_text(text) {
			// Handle NIP-42 authentication state machine on first response
			let content_for_auth = content.as_deref().unwrap_or("");
			self.handle_first_response(&kind, content_for_auth);

			// Forward the raw relay line. For frames without sub_id (AUTH/NOTICE),
			// use empty sub_id to keep the message visible upstream.
			let route_sub_id = sub_id.unwrap_or_default();
			(self.out_writer)(&self.url, &route_sub_id, text);
		} else {
			tracing::warn!(relay = %self.url, "Failed to parse incoming relay frame");
		}
	}

	fn handle_transport_status(&self, status: TransportStatus) {
		match status {
			TransportStatus::Failed { url } => {
				// During intentional close/reconnect replacement, old transport errors are expected.
				let intentional_transition = {
					let st = self.status.read().unwrap();
					matches!(*st, ConnectionStatus::Closed | ConnectionStatus::Connecting)
				};

				if intentional_transition {
					tracing::info!(relay = %url, "Transport error during intentional close/reconnect transition; ignoring failure transition");
					return;
				}

				tracing::error!(relay = %url, "Transport error");
				{
					let mut st = self.status.write().unwrap();
					*st = ConnectionStatus::Failed;
				}
				self.set_next_retry_delay_ms(RECONNECT_RETRY_DELAY_MS);
				let retry_at = *self.next_retry_at_ms.read().unwrap();
				tracing::info!(relay = %self.url, retry_at, "[connections] Connection failed, relay marked unreliable during cooldown");

				// Clear pre-auth queue on connection failure to prevent memory leak
				{
					let queue = std::mem::take(&mut *self.pre_auth_queue.write().unwrap());
					tracing::info!(relay = %self.url, cleared_frames = queue.len(), "[AUTH] Connection failed, cleared pre-auth queue");
				}
				// Set auth state to Failed until next successful reconnect path resets via first response
				*self.auth_state.write().unwrap() = AuthState::Failed;
				(self.status_writer)("failed", &url);
			}
			TransportStatus::Closed { url } => {
				let was_intentional = {
					let st = self.status.read().unwrap();
					matches!(*st, ConnectionStatus::Closed)
				};

				if was_intentional {
					tracing::info!(relay = %url, "Transport closed after intentional close");
					return;
				}

				{
					let mut st = self.status.write().unwrap();
					*st = ConnectionStatus::Closed;
				}

				// Intentional close should NOT mark relay unreliable or start cooldown.
				*self.next_retry_at_ms.write().unwrap() = 0;

				// Clear pre-auth queue on close to prevent memory leak
				{
					let queue = std::mem::take(&mut *self.pre_auth_queue.write().unwrap());
					if !queue.is_empty() {
						tracing::info!(relay = %self.url, cleared_frames = queue.len(), "[AUTH] Connection closed, cleared pre-auth queue");
					}
				}
				// Reset auth state for next transport connect.
				*self.auth_state.write().unwrap() = AuthState::Unknown;

				(self.status_writer)("close", &url);
			}
			TransportStatus::Connected { url } => {
				{
					let mut st = self.status.write().unwrap();
					*st = ConnectionStatus::Connected;
				}
				(self.status_writer)("connected", &url);
			}
		}
	}

	pub fn close_sub(&self, sub_id: &str) -> bool {
		// Fast membership check
		let present = {
			let mut set = self.active_subs.write().unwrap();
			if set.contains(sub_id) {
				set.remove(sub_id);
				true
			} else {
				false
			}
		};
		if !present {
			return false;
		}

		// Enqueue CLOSE frame; drainer will send when connected
		let frame = format!(r#"["CLOSE","{}"]"#, sub_id);
		if let Some(tx) = self.queue_tx.read().unwrap().as_ref() {
			let _ = tx.clone().try_send(frame);
		}

		true
	}

	pub fn close(&self) -> Result<(), RelayError> {
		tracing::info!(relay = %self.url, "[connections][close] closing transport intentionally (RelayConnection remains reusable)");

		// Mark as Closed first so the transport end path can distinguish intentional close.
		{
			let mut st = self.status.write().unwrap();
			*st = ConnectionStatus::Closed;
		}

		self.transport.disconnect(&self.url);

		// Intentional close should NOT mark relay unreliable or start cooldown.
		*self.next_retry_at_ms.write().unwrap() = 0;

		// Clear pre-auth queue on close to prevent memory leak
		{
			let queue = std::mem::take(&mut *self.pre_auth_queue.write().unwrap());
			if !queue.is_empty() {
				tracing::info!(relay = %self.url, cleared_frames = queue.len(), "[connections][AUTH] Connection closed, cleared pre-auth queue");
			}
		}
		// Reset auth state for next transport connect.
		*self.auth_state.write().unwrap() = AuthState::Unknown;

		(self.status_writer)("close", &self.url);

		Ok(())
	}

	/// Wake up this connection: reset backoff and trigger immediate reconnect if needed.
	/// Called when app returns from background to foreground.
	pub fn wake(self: &Arc<Self>) {
		tracing::info!(relay = %self.url, status = ?*self.status.read().unwrap(), "[connections][wake] waking connection");

		// Reset backoff to allow immediate reconnection attempt
		self.clear_backoff();

		let status = *self.status.read().unwrap();
		match status {
			ConnectionStatus::Connected => {
				tracing::info!(relay = %self.url, "[connections][wake] already connected, nothing to do");
			}
			_ => {
				// Trigger reconnection by attempting connect
				let conn = Arc::clone(self);
				spawn_worker(async move {
					tracing::info!(relay = %conn.url, "[connections][wake] triggering reconnect");
					if let Err(e) = conn.connect().await {
						tracing::warn!(relay = %conn.url, error = ?e, "[connections][wake] reconnect attempt failed");
					}
				});
			}
		}
	}

	// ------------------------------------------------------------------
	// NIP-42 Authentication handling
	// ------------------------------------------------------------------

	/// Handle the first response from relay to determine auth requirements
	fn handle_first_response(&self, kind: &str, content: &str) {
		tracing::debug!(relay = %self.url, kind, "handle_first_response called");

		// Only process if we're in Unknown state
		let is_unknown = {
			let state = self.auth_state.read().unwrap();
			let unknown = matches!(*state, AuthState::Unknown);
			tracing::debug!(relay = %self.url, state = ?*state, is_unknown = unknown, "checking auth state");
			unknown
		};

		if !is_unknown {
			// Already know auth state, check for OK after Pending
			let is_pending = {
				let state = self.auth_state.read().unwrap();
				matches!(*state, AuthState::Pending)
			};
			tracing::debug!(relay = %self.url, is_pending, "checking if pending");
			if is_pending && kind == "OK" {
				// For strict parse, content is "<accepted>[,<message>]".
				// Be conservative: only explicit true authenticates.
				let accepted =
					content.trim_start().starts_with("true") || content.trim() == "\"true\"";
				tracing::info!(relay = %self.url, accepted, content, "[connections][AUTH] OK response received");
				if accepted {
					tracing::info!(relay = %self.url, "[connections][AUTH] Accepted by relay");
					self.set_authenticated();
				} else {
					tracing::warn!(relay = %self.url, "[connections][AUTH] Rejected or non-auth OK while pending: {}", content);
				}
			}
			return;
		}

		// First response - determine auth state
		tracing::info!(relay = %self.url, kind, "[connections][AUTH] First relay response - determining auth requirements");
		match kind {
			"AUTH" => {
				// Extract challenge from AUTH response
				let challenge = content.trim_matches('"').to_string();
				tracing::info!(relay = %self.url, challenge, "[connections][AUTH] REQUIRED - challenge received");

				// Set state to Required
				*self.auth_state.write().unwrap() = AuthState::Required {
					challenge: challenge.clone(),
				};
				tracing::debug!(relay = %self.url, "[connections][AUTH] State set to Required");

				// Request signature from crypto
				self.request_auth_signature(challenge);
			}
			_ => {
				// Any other response means auth is not required
				tracing::info!(relay = %self.url, kind, "[connections][AUTH] NOT REQUIRED - relay responds with {}", kind);
				self.set_authenticated();
			}
		}
	}

	/// Send signing request to crypto worker for kind 22242 event
	fn request_auth_signature(&self, challenge: String) {
		let request_id = {
			let mut id = self.next_auth_id.write().unwrap();
			let current = *id;
			*id += 1;
			current | 0x8000_0000_0000_0000 // Mark as auth request
		};

		let created_at = now_millis();

		let payload = json!({
			"challenge": challenge,
			"relay": self.url,
			"created_at": created_at
		})
		.to_string();

		tracing::info!(relay = %self.url, request_id, payload, "[connections][AUTH] Building SignerRequest for crypto");

		// Build FlatBuffers SignerRequest
		let mut fbb = flatbuffers::FlatBufferBuilder::new();
		let payload_off = Some(fbb.create_string(&payload));

		use crate::generated::nostr::fb;
		let req = fb::SignerRequest::create(
			&mut fbb,
			&fb::SignerRequestArgs {
				request_id,
				op: fb::SignerOp::AuthEvent,
				payload: payload_off,
				pubkey: None,
				sender_pubkey: None,
				recipient_pubkey: None,
			},
		);
		fbb.finish(req, None);

		tracing::info!(relay = %self.url, request_id, bytes = fbb.finished_data().len(), "[connections][AUTH] Sending request to crypto port");

		(self.to_crypto.borrow())(fbb.finished_data());

		tracing::info!(relay = %self.url, request_id, "[connections][AUTH] Request sent to crypto successfully");
	}

	/// Handle signed auth event response from crypto
	pub fn handle_signed_auth(&self, signed_event_json: &str) {
		tracing::info!(relay = %self.url, event_len = signed_event_json.len(), "[connections][AUTH] Received signed event from crypto");
		tracing::debug!(relay = %self.url, event = signed_event_json, "[connections][AUTH] Signed event content");

		// Send AUTH to relay
		let auth_frame = format!(r#"["AUTH",{}]"#, signed_event_json);
		tracing::info!(relay = %self.url, frame = auth_frame, "[connections][AUTH] Sending frame to relay");

		// Send directly (bypass queue since this is internal)
		if let Some(tx) = self.queue_tx.read().unwrap().as_ref() {
			match tx.clone().try_send(auth_frame) {
				Ok(_) => {
					tracing::info!(relay = %self.url, "[connections][AUTH] Frame queued for relay")
				}
				Err(e) => tracing::error!(relay = %self.url, "Failed to queue AUTH frame: {:?}", e),
			}
		} else {
			tracing::error!(relay = %self.url, "No queue sender available for AUTH frame");
		}

		// Set state to Pending (waiting for OK)
		let old_state =
			std::mem::replace(&mut *self.auth_state.write().unwrap(), AuthState::Pending);
		tracing::info!(relay = %self.url, ?old_state, "[connections][AUTH] State changed to Pending (waiting for relay OK)");
	}

	/// Set authenticated state.
	///
	/// Frames are sent optimistically, so we do NOT replay any shadow queue here
	/// (avoids duplicate sends on relays that don't require auth).
	fn set_authenticated(&self) {
		let old_state = {
			let mut state = self.auth_state.write().unwrap();
			std::mem::replace(&mut *state, AuthState::Authenticated)
		};

		let dropped_shadow = {
			let queue = std::mem::take(&mut *self.pre_auth_queue.write().unwrap());
			queue.len()
		};

		tracing::info!(
			relay = %self.url,
			?old_state,
			dropped_shadow,
			"[connections][AUTH] Relay is now AUTHENTICATED (no replay; optimistic mode)"
		);
	}

	/// Public API to receive signed auth from connections main loop
	pub fn process_signed_auth(&self, signed_event_json: &str) {
		tracing::info!(relay = %self.url, "[connections][AUTH] process_signed_auth called from main loop");
		self.handle_signed_auth(signed_event_json);
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::traits::{RelayTransport, TransportError, TransportStatus};
	use async_trait::async_trait;
	use std::collections::HashMap;
	use std::sync::atomic::{AtomicUsize, Ordering};
	use std::sync::{Arc, Mutex, RwLock};
	use tokio::task::LocalSet;

	#[derive(Clone, Debug)]
	enum Call {
		Connect(String),
		Disconnect(String),
		Send(String, String),
	}

	struct MockRelayTransport {
		calls: Arc<Mutex<Vec<Call>>>,
		message_callbacks: Arc<RwLock<HashMap<String, Box<dyn Fn(String)>>>>,
		status_callbacks: Arc<RwLock<HashMap<String, Box<dyn Fn(TransportStatus)>>>>,
		connect_result: Arc<RwLock<Result<(), TransportError>>>,
		send_fail_count: Arc<AtomicUsize>,
	}

	impl MockRelayTransport {
		fn new() -> Self {
			Self {
				calls: Arc::new(Mutex::new(Vec::new())),
				message_callbacks: Arc::new(RwLock::new(HashMap::new())),
				status_callbacks: Arc::new(RwLock::new(HashMap::new())),
				connect_result: Arc::new(RwLock::new(Ok(()))),
				send_fail_count: Arc::new(AtomicUsize::new(0)),
			}
		}

		fn set_connect_result(&self, result: Result<(), TransportError>) {
			*self.connect_result.write().unwrap() = result;
		}

		fn set_send_fail_count(&self, count: usize) {
			self.send_fail_count.store(count, Ordering::SeqCst);
		}

		fn calls(&self) -> Vec<Call> {
			self.calls.lock().unwrap().clone()
		}

		fn invoke_message_callback(&self, url: &str, msg: String) {
			let cbs = self.message_callbacks.read().unwrap();
			if let Some(cb) = cbs.get(url) {
				cb(msg);
			}
		}

		fn invoke_status_callback(&self, url: &str, status: TransportStatus) {
			let cbs = self.status_callbacks.read().unwrap();
			if let Some(cb) = cbs.get(url) {
				cb(status);
			}
		}
	}

	#[async_trait(?Send)]
	impl RelayTransport for MockRelayTransport {
		async fn connect(&self, url: &str) -> Result<(), TransportError> {
			self.calls.lock().unwrap().push(Call::Connect(url.to_string()));
			self.connect_result.read().unwrap().clone()
		}

		fn disconnect(&self, url: &str) {
			self.calls.lock().unwrap().push(Call::Disconnect(url.to_string()));
		}

		async fn send(&self, url: &str, frame: String) -> Result<(), TransportError> {
			self.calls.lock().unwrap().push(Call::Send(url.to_string(), frame.clone()));
			let remaining = self.send_fail_count.load(Ordering::SeqCst);
			if remaining > 0 {
				self.send_fail_count.store(remaining - 1, Ordering::SeqCst);
				Err(TransportError::Other("send failed".to_string()))
			} else {
				Ok(())
			}
		}

		fn on_message(&self, url: &str, callback: Box<dyn Fn(String)>) {
			self.message_callbacks.write().unwrap().insert(url.to_string(), callback);
		}

		fn on_status(&self, url: &str, callback: Box<dyn Fn(TransportStatus)>) {
			self.status_callbacks.write().unwrap().insert(url.to_string(), callback);
		}
	}

	fn make_writers() -> (
		OutWriter,
		StatusWriter,
		CryptoSender,
		Arc<Mutex<Vec<(String, String, String)>>>,
		Arc<Mutex<Vec<(String, String)>>>,
		Arc<Mutex<Vec<Vec<u8>>>>,
	) {
		let out = Arc::new(Mutex::new(Vec::new()));
		let status = Arc::new(Mutex::new(Vec::new()));
		let crypto = Arc::new(Mutex::new(Vec::new()));

		let out2 = out.clone();
		let status2 = status.clone();
		let crypto2 = crypto.clone();

		let out_writer: Rc<dyn Fn(&str, &str, &str)> =
			Rc::new(move |url: &str, sub_id: &str, raw: &str| {
				out2.lock().unwrap().push((url.to_string(), sub_id.to_string(), raw.to_string()));
			});

		let status_writer: Rc<dyn Fn(&str, &str)> =
			Rc::new(move |st: &str, url: &str| {
				status2.lock().unwrap().push((st.to_string(), url.to_string()));
			});

		let to_crypto: Rc<RefCell<dyn Fn(&[u8])>> =
			Rc::new(RefCell::new(move |bytes: &[u8]| {
				crypto2.lock().unwrap().push(bytes.to_vec());
			}));

		(out_writer, status_writer, to_crypto, out, status, crypto)
	}

	#[tokio::test]
	async fn test_send_raw_queues_before_connect() {
		let local = LocalSet::new();
		local
			.run_until(async {
				let transport = Arc::new(MockRelayTransport::new());
				transport.set_connect_result(Err(TransportError::Other("not yet".to_string())));

				let (out_writer, status_writer, to_crypto, _out, _status, _crypto) =
					make_writers();

				let conn = RelayConnection::new(
					"wss://r".to_string(),
					transport.clone(),
					out_writer,
					status_writer,
					to_crypto,
				);

				// Let initial connect attempt fail
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;

				// Queue a frame before successful connection
				let frame = r#"["REQ","s1",{}]"#;
				conn.send_raw(frame).unwrap();

				// Verify send was not called yet
				let calls = transport.calls();
				assert!(
					!calls.iter().any(|c| matches!(c, Call::Send(_, _))),
					"send should not be called before connect"
				);

				// Now allow connection and wake
				transport.set_connect_result(Ok(()));
				conn.wake();

				// Let reconnect and drainer run
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;

				let calls = transport.calls();
				assert!(
					calls.iter().any(|c| matches!(c, Call::Send(url, f) if url == "wss://r" && f == frame)),
					"send should be called with queued frame after connect"
				);
			})
			.await;
	}

	#[tokio::test]
	async fn test_synthetic_subscribed_notification() {
		let local = LocalSet::new();
		local
			.run_until(async {
				let transport = Arc::new(MockRelayTransport::new());
				let (out_writer, status_writer, to_crypto, out, _status, _crypto) = make_writers();

				let _conn = RelayConnection::new(
					"wss://r".to_string(),
					transport.clone(),
					out_writer,
					status_writer,
					to_crypto,
				);

				// Let initial connect succeed and drainer start
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;

				let frame = r#"["REQ","s1",{}]"#;
				_conn.send_raw(frame).unwrap();

				// Let drainer process
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;

				let out_msgs = out.lock().unwrap();
				let subscribed = out_msgs
					.iter()
					.any(|(_, sub_id, raw)| sub_id == "s1" && raw == r#"["OK","s1","SUBSCRIBED"]"#);
				assert!(subscribed, "expected synthetic SUBSCRIBED notification");
			})
			.await;
	}

	#[tokio::test]
	async fn test_synthetic_closed_notification() {
		let local = LocalSet::new();
		local
			.run_until(async {
				let transport = Arc::new(MockRelayTransport::new());
				let (out_writer, status_writer, to_crypto, out, _status, _crypto) = make_writers();

				let conn = RelayConnection::new(
					"wss://r".to_string(),
					transport.clone(),
					out_writer,
					status_writer,
					to_crypto,
				);

				// Let initial connect succeed and drainer start
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;

				// Subscribe first
				let req_frame = r#"["REQ","s1",{}]"#;
				conn.send_raw(req_frame).unwrap();

				tokio::task::yield_now().await;
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;

				// Clear out messages to isolate CLOSE synthetic
				out.lock().unwrap().clear();

				// Now close
				let close_frame = r#"["CLOSE","s1"]"#;
				conn.send_raw(close_frame).unwrap();

				tokio::task::yield_now().await;
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;

				let out_msgs = out.lock().unwrap();
				let closed = out_msgs
					.iter()
					.any(|(_, sub_id, raw)| sub_id == "s1" && raw == r#"["OK","s1","CLOSED"]"#);
				assert!(closed, "expected synthetic CLOSED notification");
			})
			.await;
	}

	#[tokio::test]
	async fn test_reconnect_on_send_failure() {
		let local = LocalSet::new();
		local
			.run_until(async {
				let transport = Arc::new(MockRelayTransport::new());
				transport.set_send_fail_count(1);

				let (out_writer, status_writer, to_crypto, _out, _status, _crypto) = make_writers();

				let _conn = RelayConnection::new(
					"wss://r".to_string(),
					transport.clone(),
					out_writer,
					status_writer,
					to_crypto,
				);

				// Let initial connect succeed and drainer start
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;

				let frame = r#"["REQ","s1",{}]"#;
				_conn.send_raw(frame).unwrap();

				// Let drainer process: send fails, then reconnect, then retry succeeds
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;

				let calls = transport.calls();
				let connect_count = calls.iter().filter(|c| matches!(c, Call::Connect(_))).count();
				assert!(
					connect_count >= 2,
					"expected at least 2 connect calls (initial + reconnect), got {}",
					connect_count
				);
			})
			.await;
	}

	#[tokio::test]
	async fn test_auth_state_machine() {
		let local = LocalSet::new();
		local
			.run_until(async {
				let transport = Arc::new(MockRelayTransport::new());
				let (out_writer, status_writer, to_crypto, _out, _status, crypto) = make_writers();

				let conn = RelayConnection::new(
					"wss://r".to_string(),
					transport.clone(),
					out_writer,
					status_writer,
					to_crypto,
				);

				// Let initial connect succeed and callbacks register
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;

				// Inject AUTH challenge from relay
				transport.invoke_message_callback(
					"wss://r",
					r#"["AUTH","challenge123"]"#.to_string(),
				);

				// Let auth handling run
				tokio::task::yield_now().await;

				// Verify auth state is Required
				{
					let state = conn.auth_state.read().unwrap();
					assert!(
						matches!(*state, AuthState::Required { .. }),
						"expected Required auth state after AUTH, got {:?}",
						*state
					);
				}

				// Verify crypto request was sent
				let crypto_msgs = crypto.lock().unwrap();
				assert!(!crypto_msgs.is_empty(), "expected crypto request to be sent");
				drop(crypto_msgs);

				// Process signed auth response
				let signed_event = r#"{"id":"abc","pubkey":"pk","created_at":123,"kind":22242,"tags":[["challenge","challenge123"],["relay","wss://r"]],"content":"","sig":"sig"}"#;
				conn.process_signed_auth(signed_event);

				// Verify state becomes Pending
				{
					let state = conn.auth_state.read().unwrap();
					assert!(
						matches!(*state, AuthState::Pending),
						"expected Pending auth state after process_signed_auth, got {:?}",
						*state
					);
				}

				// Let drainer send the AUTH frame
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;

				// Inject OK response from relay
				transport.invoke_message_callback(
					"wss://r",
					r#"["OK","auth-id","true"]"#.to_string(),
				);

				// Let auth handling run
				tokio::task::yield_now().await;

				// Verify state becomes Authenticated
				{
					let state = conn.auth_state.read().unwrap();
					assert!(
						matches!(*state, AuthState::Authenticated),
						"expected Authenticated auth state after OK, got {:?}",
						*state
					);
				}
			})
			.await;
	}

	#[tokio::test]
	async fn test_close_disconnects_transport() {
		let local = LocalSet::new();
		local
			.run_until(async {
				let transport = Arc::new(MockRelayTransport::new());
				let (out_writer, status_writer, to_crypto, _out, _status, _crypto) = make_writers();

				let conn = RelayConnection::new(
					"wss://r".to_string(),
					transport.clone(),
					out_writer,
					status_writer,
					to_crypto,
				);

				// Let initial connect succeed
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;

				conn.close().unwrap();

				let calls = transport.calls();
				assert!(
					calls.iter().any(|c| matches!(c, Call::Disconnect(url) if url == "wss://r")),
					"expected disconnect to be called on close"
				);
			})
			.await;
	}

	#[tokio::test]
	async fn test_intentional_close_does_not_trigger_cooldown() {
		let local = LocalSet::new();
		local
			.run_until(async {
				let transport = Arc::new(MockRelayTransport::new());
				let (out_writer, status_writer, to_crypto, _out, _status, _crypto) = make_writers();

				let conn = RelayConnection::new(
					"wss://r".to_string(),
					transport.clone(),
					out_writer,
					status_writer,
					to_crypto,
				);

				// Let initial connect succeed
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;

				conn.close().unwrap();

				assert_eq!(
					*conn.next_retry_at_ms.read().unwrap(),
					0,
					"expected no cooldown after intentional close"
				);
			})
			.await;
	}

	#[tokio::test]
	async fn test_no_reconnect_after_explicit_close() {
		let local = LocalSet::new();
		local
			.run_until(async {
				let transport = Arc::new(MockRelayTransport::new());
				let (out_writer, status_writer, to_crypto, _out, _status, _crypto) = make_writers();

				let conn = RelayConnection::new(
					"wss://r".to_string(),
					transport.clone(),
					out_writer,
					status_writer,
					to_crypto,
				);

				// Let initial connect succeed
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;

				conn.close().unwrap();

				// Simulate transport failure after intentional close
				transport.invoke_status_callback(
					"wss://r",
					TransportStatus::Failed {
						url: "wss://r".to_string(),
					},
				);
				tokio::task::yield_now().await;

				// Status should remain Closed, not transition to Failed/cooldown
				assert!(
					matches!(*conn.status.read().unwrap(), ConnectionStatus::Closed),
					"expected status to remain Closed after transport failure post-close"
				);
				assert_eq!(
					*conn.next_retry_at_ms.read().unwrap(),
					0,
					"expected no cooldown when transport fails after explicit close"
				);
			})
			.await;
	}

	#[tokio::test]
	async fn test_cooldown_respected_after_failure() {
		let local = LocalSet::new();
		local
			.run_until(async {
				let transport = Arc::new(MockRelayTransport::new());
				let (out_writer, status_writer, to_crypto, _out, _status, _crypto) = make_writers();

				let conn = RelayConnection::new(
					"wss://r".to_string(),
					transport.clone(),
					out_writer,
					status_writer,
					to_crypto,
				);

				// Let initial connect succeed
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;

				// Inject transport failure to trigger cooldown
				transport.invoke_status_callback(
					"wss://r",
					TransportStatus::Failed {
						url: "wss://r".to_string(),
					},
				);
				tokio::task::yield_now().await;

				// Verify cooldown is set
				let retry_at = *conn.next_retry_at_ms.read().unwrap();
				assert!(
					retry_at > now_millis(),
					"expected cooldown to be set, retry_at={} now={}",
					retry_at,
					now_millis()
				);

				// Verify connect is blocked by cooldown
				let result = conn.connect().await;
				assert!(
					matches!(result, Err(RelayError::ConnectionClosed)),
					"expected connect to be blocked by cooldown, got {:?}",
					result
				);
			})
			.await;
	}

	#[tokio::test]
	async fn test_staged_reconnect_with_delays() {
		let local = LocalSet::new();
		local
			.run_until(async {
				let transport = Arc::new(MockRelayTransport::new());
				let (out_writer, status_writer, to_crypto, _out, _status, _crypto) = make_writers();

				let conn = RelayConnection::new(
					"wss://r".to_string(),
					transport.clone(),
					out_writer,
					status_writer,
					to_crypto,
				);

				// Let initial connect succeed
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;

				// Make subsequent connects fail and the first send fail
				transport.set_connect_result(Err(TransportError::Other("fail".to_string())));
				transport.set_send_fail_count(1);

				// Inject transport failure so status becomes Failed (cold-start delays)
				transport.invoke_status_callback(
					"wss://r",
					TransportStatus::Failed {
						url: "wss://r".to_string(),
					},
				);
				tokio::task::yield_now().await;

				// Clear cooldown so drainer isn't blocked, but keep Failed status for cold-start delays
				*conn.next_retry_at_ms.write().unwrap() = 0;

				conn.send_raw(r#"["REQ","s1",{}]"#).unwrap();

				// Let drainer process direct send failure and reconnect #1 failure
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;

				// Wait for the single 200ms cold-start staged delay
				tokio::time::sleep(std::time::Duration::from_millis(250)).await;
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;

				let calls = transport.calls();
				let connect_count = calls.iter().filter(|c| matches!(c, Call::Connect(_))).count();
				assert!(
					connect_count >= 3,
					"expected at least 3 connect attempts (initial + reconnect + staged), got {}",
					connect_count
				);

				// Verify cooldown was set after exhaustion
				let retry_at = *conn.next_retry_at_ms.read().unwrap();
				assert!(
					retry_at > 0,
					"expected cooldown after staged reconnect exhaustion"
				);
			})
			.await;
	}

	#[tokio::test]
	async fn test_close_sub_removes_subscription_and_sends_close() {
		let local = LocalSet::new();
		local
			.run_until(async {
				let transport = Arc::new(MockRelayTransport::new());
				let (out_writer, status_writer, to_crypto, out, _status, _crypto) = make_writers();

				let conn = RelayConnection::new(
					"wss://r".to_string(),
					transport.clone(),
					out_writer,
					status_writer,
					to_crypto,
				);

				// Let initial connect succeed
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;

				// Subscribe
				let req_frame = r#"["REQ","s1",{}]"#;
				conn.send_raw(req_frame).unwrap();
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;

				// Verify sub is active
				assert!(conn.active_subs.read().unwrap().contains("s1"));

				// Close the sub (sync, no await)
				let result = conn.close_sub("s1");
				assert!(result, "close_sub should return true for active sub");

				// Let drainer process the CLOSE frame
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;

				// Verify sub removed
				assert!(!conn.active_subs.read().unwrap().contains("s1"));

				// Verify a Send call with CLOSE was made
				let calls = transport.calls();
				let close_sent = calls.iter().any(|c| {
					matches!(c, Call::Send(url, f) if url == "wss://r" && f == r#"["CLOSE","s1"]"#)
				});
				assert!(close_sent, "expected CLOSE frame to be sent to relay");

				// Verify synthetic CLOSED notification
				let closed = out.lock().unwrap().iter().any(|(_, sub_id, raw)| {
					sub_id == "s1" && raw == r#"["OK","s1","CLOSED"]"#
				});
				assert!(closed, "expected synthetic CLOSED notification");
			})
			.await;
	}
}
