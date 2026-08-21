#[cfg(target_os = "android")]
mod jni;
mod mesh_ffi;

use futures::StreamExt;
use nipworker_core::service::engine::NostrEngine;
use nipworker_core::storage::{NostrDbStorage, PersistentNostrDbStorage};
use std::cell::UnsafeCell;
use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr};
use std::path::PathBuf;
use std::slice;
use std::sync::{
    atomic::{AtomicU8, Ordering},
    Arc, Condvar, Mutex, OnceLock,
};
use std::thread;
use tokio::runtime::Builder;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::LocalSet;

pub mod storage;
pub mod transport;

use storage::FileBlobStore;
use transport::NativeTransport;

const DEFAULT_RELAYS: &[&str] = &[
    "wss://relay.snort.social",
    "wss://relay.damus.io",
    "wss://relay.primal.net",
];
const INDEXER_RELAYS: &[&str] = &[
    "wss://user.kindpag.es",
    "wss://relay.nos.social",
    "wss://purplepag.es",
    "wss://profiles.nostr1.com",
];

const LOG_LEVEL_TRACE: u8 = 0;
const LOG_LEVEL_DEBUG: u8 = 1;
const LOG_LEVEL_INFO: u8 = 2;
const LOG_LEVEL_WARN: u8 = 3;
const LOG_LEVEL_ERROR: u8 = 4;
static NATIVE_LOG_LEVEL: AtomicU8 = AtomicU8::new(LOG_LEVEL_WARN);

fn parse_native_log_level(level: &str) -> u8 {
    match level.trim().to_ascii_lowercase().as_str() {
        "trace" => LOG_LEVEL_TRACE,
        "debug" => LOG_LEVEL_DEBUG,
        "info" => LOG_LEVEL_INFO,
        "warn" => LOG_LEVEL_WARN,
        "error" => LOG_LEVEL_ERROR,
        _ => LOG_LEVEL_ERROR,
    }
}

#[cfg(any(target_vendor = "apple", target_os = "android"))]
fn native_log_enabled(level: &tracing::Level) -> bool {
    let event_level = match *level {
        tracing::Level::TRACE => LOG_LEVEL_TRACE,
        tracing::Level::DEBUG => LOG_LEVEL_DEBUG,
        tracing::Level::INFO => LOG_LEVEL_INFO,
        tracing::Level::WARN => LOG_LEVEL_WARN,
        tracing::Level::ERROR => LOG_LEVEL_ERROR,
    };
    event_level >= NATIVE_LOG_LEVEL.load(Ordering::Relaxed)
}

/// Sets the process-wide native log filter. The filter remains reloadable after
/// engine initialization so a shared React Native runtime can update its level.
#[no_mangle]
pub extern "C" fn nipworker_set_log_level(level: *const c_char) {
    let parsed = if level.is_null() {
        LOG_LEVEL_ERROR
    } else {
        let value = unsafe { CStr::from_ptr(level) }.to_string_lossy();
        parse_native_log_level(&value)
    };
    NATIVE_LOG_LEVEL.store(parsed, Ordering::Relaxed);
}

fn split_relay_csv(value: *const c_char) -> Vec<String> {
    if value.is_null() {
        return Vec::new();
    }
    unsafe { CStr::from_ptr(value) }
        .to_string_lossy()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn fallback_relays(relays: Vec<String>, fallback: &[&str]) -> Vec<String> {
    if relays.is_empty() {
        fallback.iter().map(|s| s.to_string()).collect()
    } else {
        relays
    }
}

fn new_core_storage(
    max_buffer_size: usize,
    default_relays: Vec<String>,
    indexer_relays: Vec<String>,
) -> NostrDbStorage {
    new_named_core_storage("nipworker", max_buffer_size, default_relays, indexer_relays)
}

fn new_named_core_storage(
    name: &str,
    max_buffer_size: usize,
    default_relays: Vec<String>,
    indexer_relays: Vec<String>,
) -> NostrDbStorage {
    NostrDbStorage::new(
        name.to_string(),
        max_buffer_size,
        fallback_relays(default_relays, DEFAULT_RELAYS),
        fallback_relays(indexer_relays, INDEXER_RELAYS),
    )
}

/// Commands sent to the engine thread
enum EngineCommand {
    HandleMessage(Vec<u8>),
    ClearSigner,
    RemoveSigner,
    Wake,
    Shutdown,
}

/// Fixed allocation shared between the subscription store and any JSI
/// ArrayBuffers that currently expose it. The allocation never resizes or
/// moves. Rust-side mutation is serialized by `NativeSubscriptionStore`'s
/// mutex; foreign readers observe the producer-published write cursor and use
/// `nipworker_subscription_try_reset` for the drain/reset race.
struct NativeSubscriptionBuffer {
    bytes: UnsafeCell<Box<[u8]>>,
}

// `UnsafeCell` avoids another mutex on the hot append path. All Rust mutation
// occurs while holding the enclosing store mutex, and the C/JSI consumer never
// creates Rust references to the allocation.
unsafe impl Send for NativeSubscriptionBuffer {}
unsafe impl Sync for NativeSubscriptionBuffer {}

impl NativeSubscriptionBuffer {
    fn new(size: usize) -> Self {
        let mut bytes = vec![0; size].into_boxed_slice();
        bytes[0..4].copy_from_slice(&4u32.to_le_bytes());
        Self {
            bytes: UnsafeCell::new(bytes),
        }
    }

    fn len(&self) -> usize {
        // The boxed slice length is immutable for the backing's lifetime.
        unsafe { (&*self.bytes.get()).len() }
    }

    fn as_mut_ptr(&self) -> *mut u8 {
        // The boxed allocation is fixed and remains alive while an Arc exists.
        unsafe { (&mut *self.bytes.get()).as_mut_ptr() }
    }

    fn bytes(&self) -> &[u8] {
        unsafe { &*self.bytes.get() }
    }

    unsafe fn bytes_mut(&self) -> &mut [u8] {
        unsafe { &mut *self.bytes.get() }
    }
}

struct NativeSubscription {
    backing: Arc<NativeSubscriptionBuffer>,
    ref_count: i32,
    close_on_cleanup: bool,
}

impl NativeSubscription {
    fn new(buffer_size: usize, close_on_cleanup: bool) -> Self {
        let size = buffer_size.max(4);
        Self {
            backing: Arc::new(NativeSubscriptionBuffer::new(size)),
            ref_count: 1,
            close_on_cleanup,
        }
    }

    fn write_pos(&self) -> u32 {
        let buffer = self.backing.bytes();
        u32::from_le_bytes([buffer[0], buffer[1], buffer[2], buffer[3]])
    }

    /// Reset the write cursor back to 4 so drained space can be reused, but only
    /// if the cursor still matches `expected_write_pos` (i.e. the engine has not
    /// appended since the host last read).
    fn try_reset(&mut self, expected_write_pos: u32) -> bool {
        if self.write_pos() != expected_write_pos {
            return false;
        }
        let buffer = unsafe { self.backing.bytes_mut() };
        buffer[0..4].copy_from_slice(&4u32.to_le_bytes());
        true
    }

    fn append_payload(&mut self, payload: &[u8]) -> bool {
        let write_pos = self.write_pos() as usize;
        let required = 4 + payload.len();
        if write_pos + required > self.backing.len() {
            return false;
        }
        let buffer = unsafe { self.backing.bytes_mut() };
        buffer[write_pos..write_pos + 4].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        buffer[write_pos + 4..write_pos + 4 + payload.len()].copy_from_slice(payload);
        let next_pos = (write_pos + required) as u32;
        buffer[0..4].copy_from_slice(&next_pos.to_le_bytes());
        true
    }

    #[cfg(test)]
    fn bytes(&self) -> &[u8] {
        self.backing.bytes()
    }
}

struct NativeSubscriptionStore {
    subscriptions: HashMap<String, NativeSubscription>,
}

enum AppendOutcome {
    Written,
    Full { close_on_cleanup: bool },
    Missing,
}

impl NativeSubscriptionStore {
    fn new() -> Self {
        Self {
            subscriptions: HashMap::new(),
        }
    }

    fn register(&mut self, sub_id: String, buffer_size: usize, close_on_cleanup: bool) {
        if let Some(existing) = self.subscriptions.get_mut(&sub_id) {
            existing.ref_count += 1;
            return;
        }
        self.subscriptions.insert(
            sub_id,
            NativeSubscription::new(buffer_size, close_on_cleanup),
        );
    }

    fn retain(&mut self, sub_id: &str) -> bool {
        if let Some(existing) = self.subscriptions.get_mut(sub_id) {
            existing.ref_count += 1;
            return true;
        }
        false
    }

    fn release(&mut self, sub_id: &str) {
        if let Some(existing) = self.subscriptions.get_mut(sub_id) {
            existing.ref_count -= 1;
        }
    }

    fn append_payload(&mut self, sub_id: &str, payload: &[u8]) -> AppendOutcome {
        if let Some(existing) = self.subscriptions.get_mut(sub_id) {
            if existing.append_payload(payload) {
                return AppendOutcome::Written;
            }
            return AppendOutcome::Full {
                close_on_cleanup: existing.close_on_cleanup,
            };
        }
        AppendOutcome::Missing
    }
}

struct NipworkerState {
    destroyed: bool,
    alive: Arc<std::sync::atomic::AtomicBool>,
    cmd_tx: Option<UnboundedSender<EngineCommand>>,
    subscriptions: Arc<Mutex<NativeSubscriptionStore>>,
    mesh_tx: Option<tokio::sync::mpsc::UnboundedSender<mesh_ffi::MeshCommand>>,
    engine_thread: Option<thread::JoinHandle<()>>,
}

/// Runtime-independent owner for a pinned subscription buffer. Unlike the
/// legacy retain/ptr pair, this token never dereferences the engine handle when
/// the JSI ArrayBuffer is eventually finalized after a runtime reload.
struct NipworkerSubscriptionPin {
    _backing: Arc<NativeSubscriptionBuffer>,
}

/// Opaque handle
pub struct NipworkerHandle {
    state: Mutex<NipworkerState>,
}

type NativeCallback = extern "C" fn(*mut c_void, *const u8, usize);

#[derive(Clone, Copy)]
struct SharedClient {
    callback: NativeCallback,
    userdata: usize,
    references: usize,
}

#[derive(Default)]
struct SharedEngineState {
    handle: usize,
    generation: u64,
    process_anchored: bool,
    clients: Vec<SharedClient>,
    callbacks_in_flight: usize,
}

static SHARED_ENGINE: OnceLock<(Mutex<SharedEngineState>, Condvar)> = OnceLock::new();
static SHARED_ENGINE_LIFECYCLE: Mutex<()> = Mutex::new(());

#[cfg(test)]
static SHARED_ENGINE_INITIALIZATIONS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

fn shared_engine() -> &'static (Mutex<SharedEngineState>, Condvar) {
    SHARED_ENGINE.get_or_init(|| (Mutex::new(SharedEngineState::default()), Condvar::new()))
}

fn shared_reference_count(state: &SharedEngineState) -> usize {
    state.clients.iter().map(|client| client.references).sum()
}

extern "C" fn shared_engine_callback(_userdata: *mut c_void, ptr: *const u8, len: usize) {
    let clients = {
        let (mutex, _) = shared_engine();
        let Ok(mut state) = mutex.lock() else {
            unsafe { nipworker_free_bytes(ptr as *mut u8, len) };
            return;
        };
        if state.clients.is_empty() {
            drop(state);
            unsafe { nipworker_free_bytes(ptr as *mut u8, len) };
            return;
        }
        state.callbacks_in_flight += 1;
        state.clients.clone()
    };

    // Preserve the original owned allocation for the first consumer. Copies
    // are allocated before invoking it because callbacks own and may
    // immediately free their packet. The normal one-client path remains
    // allocation/copy free across this registry.
    let copies = if clients.len() > 1 && !ptr.is_null() && len > 0 {
        let bytes = unsafe { slice::from_raw_parts(ptr, len) };
        (1..clients.len())
            .map(|_| Box::into_raw(bytes.to_vec().into_boxed_slice()) as *const u8)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    (clients[0].callback)(clients[0].userdata as *mut c_void, ptr, len);
    for (client, copy) in clients.iter().skip(1).zip(copies) {
        (client.callback)(client.userdata as *mut c_void, copy, len);
    }

    let (mutex, changed) = shared_engine();
    if let Ok(mut state) = mutex.lock() {
        state.callbacks_in_flight = state.callbacks_in_flight.saturating_sub(1);
        changed.notify_all();
    }
}

/// Build a FlatBuffers MainMessage that contains a SetSigner(PrivateKey) payload.
fn build_set_private_key_message(secret: &str) -> Vec<u8> {
    use flatbuffers::FlatBufferBuilder;
    use nipworker_core::generated::nostr::fb;

    let mut builder = FlatBufferBuilder::new();
    let mut pk = fb::PrivateKeyT::default();
    pk.private_key = secret.to_string();
    let signer_type = fb::SignerTypeT::PrivateKey(Box::new(pk));
    let signer_offset = signer_type.pack(&mut builder);
    let set_signer = fb::SetSigner::create(
        &mut builder,
        &fb::SetSignerArgs {
            signer_type_type: fb::SignerType::PrivateKey,
            signer_type: signer_offset,
        },
    );
    let main_msg = fb::MainMessage::create(
        &mut builder,
        &fb::MainMessageArgs {
            content_type: fb::MainContent::SetSigner,
            content: Some(set_signer.as_union_value()),
        },
    );
    builder.finish(main_msg, None);
    builder.finished_data().to_vec()
}

fn build_unsubscribe_message(subscription_id: &str) -> Vec<u8> {
    use flatbuffers::FlatBufferBuilder;
    use nipworker_core::generated::nostr::fb;

    let mut builder = FlatBufferBuilder::new();
    let sub_id_offset = builder.create_string(subscription_id);
    let unsubscribe = fb::Unsubscribe::create(
        &mut builder,
        &fb::UnsubscribeArgs {
            subscription_id: Some(sub_id_offset),
        },
    );
    let main_msg = fb::MainMessage::create(
        &mut builder,
        &fb::MainMessageArgs {
            content_type: fb::MainContent::Unsubscribe,
            content: Some(unsubscribe.as_union_value()),
        },
    );
    builder.finish(main_msg, None);
    builder.finished_data().to_vec()
}

const ROUTE_WAKE_MAGIC: &[u8; 4] = b"NWR1";

fn build_route_wake_frame(sub_id: &str) -> Vec<u8> {
    let sub_id_bytes = sub_id.as_bytes();
    let mut frame = Vec::with_capacity(8 + sub_id_bytes.len());
    frame.extend_from_slice(ROUTE_WAKE_MAGIC);
    frame.extend_from_slice(&(sub_id_bytes.len() as u32).to_le_bytes());
    frame.extend_from_slice(sub_id_bytes);
    frame
}

/// One callback emission derived from a drained batch of async events.
enum CallbackAction {
    /// Direct callback payload (empty sub_id or "crypto") delivered as-is.
    Payload(Vec<u8>),
    /// Route wake telling the host that `sub_id`'s buffer has new data.
    Wake(String),
}

/// Result of applying a drained batch of async events to the subscription store.
struct BatchOutcome {
    /// Callbacks to emit, in channel order with at most one wake per sub_id.
    actions: Vec<CallbackAction>,
    /// Sub ids whose buffer filled up with `close_on_cleanup` set; the caller
    /// should send an unsubscribe EngineCommand for each.
    unsubscribes: Vec<String>,
    /// Payloads rejected because their fixed-capacity subscription buffer was
    /// full or their subscription had already been removed.
    dropped_events: usize,
    dropped_bytes: usize,
    /// Subset of dropped events targeting a subscription that no longer exists.
    missing_events: usize,
    missing_bytes: usize,
}

/// Append a drained batch of async events to their subscription buffers and
/// coalesce the resulting callbacks: one route wake per sub_id for the whole
/// batch, while crypto/empty-sub messages pass through individually.
/// Appends happen in channel order; wakes point at data already appended.
fn apply_event_batch(
    subscriptions: &mut NativeSubscriptionStore,
    batch: Vec<(String, Vec<u8>)>,
) -> BatchOutcome {
    let mut actions = Vec::with_capacity(batch.len());
    let mut unsubscribes = Vec::new();
    let mut unsubscribe_ids = std::collections::HashSet::new();
    let mut full_ids = std::collections::HashSet::new();
    let mut missing_ids = std::collections::HashSet::new();
    let mut woken = std::collections::HashSet::new();
    let mut dropped_events = 0usize;
    let mut dropped_bytes = 0usize;
    let mut missing_events = 0usize;
    let mut missing_bytes = 0usize;
    for (sub_id, bytes) in batch {
        // Direct crypto responses are delivered as callback payloads and do not
        // own a registered subscription buffer.
        if sub_id.is_empty() || sub_id == "crypto" {
            actions.push(CallbackAction::Payload(bytes));
            continue;
        }
        match subscriptions.append_payload(&sub_id, &bytes) {
            AppendOutcome::Written => {
                if woken.insert(sub_id.clone()) {
                    actions.push(CallbackAction::Wake(sub_id));
                }
            }
            AppendOutcome::Full { close_on_cleanup } => {
                dropped_events += 1;
                dropped_bytes = dropped_bytes.saturating_add(bytes.len());
                if full_ids.insert(sub_id.clone()) {
                    log::warn!(
                        "[nipworker-native] native buffer full for subId={} (subIdLen={}, payloadLen={})",
                        sub_id,
                        sub_id.len(),
                        bytes.len()
                    );
                }
                if close_on_cleanup && unsubscribe_ids.insert(sub_id.clone()) {
                    unsubscribes.push(sub_id);
                }
            }
            AppendOutcome::Missing => {
                dropped_events += 1;
                dropped_bytes = dropped_bytes.saturating_add(bytes.len());
                missing_events += 1;
                missing_bytes = missing_bytes.saturating_add(bytes.len());
                if missing_ids.insert(sub_id.clone()) {
                    log::debug!(
                        "[nipworker-native] dropping event for missing subscription subId={} (subIdLen={}, payloadLen={})",
                        sub_id,
                        sub_id.len(),
                        bytes.len()
                    );
                }
            }
        }
    }
    BatchOutcome {
        actions,
        unsubscribes,
        dropped_events,
        dropped_bytes,
        missing_events,
        missing_bytes,
    }
}

fn subscription_buffer_size_from_message(bytes: &[u8]) -> Option<(String, usize)> {
    let main_message =
        flatbuffers::root::<nipworker_core::generated::nostr::fb::MainMessage>(bytes).ok()?;
    let subscribe = main_message.content_as_subscribe()?;
    let total_limit = subscribe
        .requests()
        .iter()
        .map(|request| {
            let limit = request.limit();
            if limit > 0 {
                limit as usize
            } else {
                100
            }
        })
        .sum::<usize>()
        .max(1);
    let bytes_per_event = match subscribe.config().bytes_per_event() {
        0 => 3072usize,
        value => value as usize,
    };
    let data_size = total_limit.saturating_mul(bytes_per_event);
    let overhead = data_size / 4;
    Some((
        subscribe.subscription_id().to_string(),
        4usize.saturating_add(data_size).saturating_add(overhead),
    ))
}

fn publish_id_from_message(bytes: &[u8]) -> Option<String> {
    let main_message =
        flatbuffers::root::<nipworker_core::generated::nostr::fb::MainMessage>(bytes).ok()?;
    let publish = main_message.content_as_publish()?;
    Some(publish.publish_id().to_string())
}

#[no_mangle]
pub extern "C" fn nipworker_init(
    callback: extern "C" fn(*mut c_void, *const u8, usize),
    userdata: *mut c_void,
) -> *mut c_void {
    nipworker_init_with_storage_path(callback, userdata, std::ptr::null())
}

#[no_mangle]
pub extern "C" fn nipworker_init_with_storage_path(
    callback: extern "C" fn(*mut c_void, *const u8, usize),
    userdata: *mut c_void,
    storage_path: *const c_char,
) -> *mut c_void {
    nipworker_init_with_config(
        callback,
        userdata,
        storage_path,
        std::ptr::null(),
        std::ptr::null(),
    )
}

#[no_mangle]
pub extern "C" fn nipworker_init_with_config(
    callback: extern "C" fn(*mut c_void, *const u8, usize),
    userdata: *mut c_void,
    storage_path: *const c_char,
    default_relays: *const c_char,
    indexer_relays: *const c_char,
) -> *mut c_void {
    nipworker_init_with_options(
        callback,
        userdata,
        storage_path,
        default_relays,
        indexer_relays,
        false,
    )
}

#[no_mangle]
pub extern "C" fn nipworker_init_with_options(
    callback: extern "C" fn(*mut c_void, *const u8, usize),
    userdata: *mut c_void,
    storage_path: *const c_char,
    default_relays: *const c_char,
    indexer_relays: *const c_char,
    mesh_enabled: bool,
) -> *mut c_void {
    // Initialize tracing subscriber for native builds
    #[cfg(target_vendor = "apple")]
    {
        use tracing_subscriber::prelude::*;
        let _ = tracing_log::LogTracer::init();
        log::set_max_level(log::LevelFilter::Trace);
        let filter =
            tracing_subscriber::filter::filter_fn(|metadata| native_log_enabled(metadata.level()));
        let _ = tracing_subscriber::registry()
            .with(
                tracing_oslog::OsLogger::new("com.nutscash.sparkling", "nipworker")
                    .with_filter(filter),
            )
            .try_init();
    }
    #[cfg(target_os = "android")]
    {
        // Route both tracing and log-facade records through one reloadable
        // filter, then write directly to liblog to avoid a tracing/log cycle.
        use tracing_subscriber::prelude::*;
        const NIPWORKER_TAG: &[u8] = b"nipworker\0";
        struct LogcatWriter;
        impl std::io::Write for LogcatWriter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                let msg = String::from_utf8_lossy(buf);
                let msg = msg.trim_end().replace('\0', "?");
                match std::ffi::CString::new(msg) {
                    Ok(c) => unsafe {
                        android_log_sys::__android_log_write(
                            android_log_sys::LogPriority::INFO as std::ffi::c_int,
                            NIPWORKER_TAG.as_ptr().cast(),
                            c.as_ptr(),
                        );
                    },
                    Err(_) => unsafe {
                        android_log_sys::__android_log_write(
                            android_log_sys::LogPriority::INFO as std::ffi::c_int,
                            NIPWORKER_TAG.as_ptr().cast(),
                            b"<log message not representable>\0".as_ptr().cast(),
                        );
                    },
                }
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        struct LogcatMakeWriter;
        impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogcatMakeWriter {
            type Writer = LogcatWriter;
            fn make_writer(&'a self) -> Self::Writer {
                LogcatWriter
            }
        }
        let _ = tracing_log::LogTracer::init();
        log::set_max_level(log::LevelFilter::Trace);
        let filter =
            tracing_subscriber::filter::filter_fn(|metadata| native_log_enabled(metadata.level()));
        let layer = tracing_subscriber::fmt::layer()
            .with_writer(LogcatMakeWriter)
            .with_ansi(false)
            .with_filter(filter);
        let _ = tracing_subscriber::registry().with(layer).try_init();
    }
    #[cfg(all(not(target_vendor = "apple"), not(target_os = "android")))]
    {
        let _ = tracing_log::LogTracer::init();
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::ERROR)
            .with_ansi(false)
            .try_init();
    }

    let storage_path = if storage_path.is_null() {
        None
    } else {
        let path = unsafe { CStr::from_ptr(storage_path) }
            .to_string_lossy()
            .to_string();
        if path.is_empty() {
            None
        } else {
            Some(PathBuf::from(path))
        }
    };
    let default_relays = split_relay_csv(default_relays);
    let indexer_relays = split_relay_csv(indexer_relays);

    // Set panic hook so Rust panics are visible instead of silent thread death
    std::panic::set_hook(Box::new(|info| {
        let backtrace = std::backtrace::Backtrace::capture();
        eprintln!("[nipworker] PANIC: {}", info);
        eprintln!("[nipworker] Backtrace:\n{}", backtrace);
    }));

    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<EngineCommand>();
    let (mesh_tx, mesh_rx) = if mesh_enabled {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<mesh_ffi::MeshCommand>();
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };
    let subscriptions = Arc::new(Mutex::new(NativeSubscriptionStore::new()));
    let alive = Arc::new(std::sync::atomic::AtomicBool::new(true));

    // Cast userdata to usize so it can be moved into the spawned thread.
    let userdata = userdata as usize;
    let callback_subscriptions = subscriptions.clone();
    let callback_cmd_tx = cmd_tx.clone();
    let callback_alive = alive.clone();

    // Spawn engine thread
    let engine_thread = thread::Builder::new()
        .name("nipworker-engine".to_string())
        .spawn(move || {
        let rt = Builder::new_current_thread().enable_all().build().unwrap();

        let local = LocalSet::new();

        local.spawn_local(async move {
            let (async_event_tx, mut async_event_rx) =
                futures::channel::mpsc::channel::<(String, Vec<u8>)>(256);

            let client_storage_path = storage_path.clone();
            let mesh_storage_path = storage_path.clone();
            let mesh_default_relays = default_relays.clone();
            let mesh_indexer_relays = indexer_relays.clone();
            let client_storage_factory = move || {
                    if let Some(path) = client_storage_path.clone() {
                        Arc::new(PersistentNostrDbStorage::new(
                            new_core_storage(
                                8 * 1024 * 1024,
                                default_relays.clone(),
                                indexer_relays.clone(),
                            ),
                            FileBlobStore::new(path),
                        )) as Arc<dyn nipworker_core::traits::Storage>
                    } else {
                        Arc::new(new_core_storage(
                            8 * 1024 * 1024,
                            default_relays.clone(),
                            indexer_relays.clone(),
                        )) as Arc<dyn nipworker_core::traits::Storage>
                    }
                };
            let engine = if let Some(mesh_rx) = mesh_rx {
                let (engine, mesh_endpoint) = NostrEngine::new_threaded_with_mesh(
                    || Arc::new(NativeTransport::new()),
                    client_storage_factory,
                    move || {
                    let storage = new_named_core_storage(
                        "nipworker-mesh",
                        8 * 1024 * 1024,
                        mesh_default_relays.clone(),
                        mesh_indexer_relays.clone(),
                    );
                    if let Some(path) = mesh_storage_path.clone() {
                        Arc::new(PersistentNostrDbStorage::new(storage, FileBlobStore::new(path)))
                            as Arc<dyn nipworker_core::traits::Storage>
                    } else {
                        Arc::new(storage) as Arc<dyn nipworker_core::traits::Storage>
                    }
                },
                    async_event_tx,
                );
                tokio::task::spawn_local(mesh_ffi::run_mesh_runtime(mesh_endpoint, mesh_rx));
                Arc::new(engine)
            } else {
                Arc::new(NostrEngine::new_threaded(
                    || Arc::new(NativeTransport::new()),
                    client_storage_factory,
                    async_event_tx,
                ))
            };

            // Bridge async events to the native transport callback. The
            // callback receives an owned allocation and may adopt it directly
            // into its bounded queue; it must eventually call
            // nipworker_free_bytes(), but no intermediate copy is required.
            // Pending events are drained and coalesced so a burst emits a
            // single route wake per sub_id instead of one FFI call per event.
            tokio::task::spawn_local(async move {
                while let Some(first) = async_event_rx.next().await {
                    let mut batch = vec![first];
                    while let Ok(pending) = async_event_rx.try_recv() {
                        batch.push(pending);
                    }
                    let batch_len = batch.len();
                    let batch_bytes = batch
                        .iter()
                        .fold(0usize, |total, (_, bytes)| total.saturating_add(bytes.len()));
                    let outcome = match callback_subscriptions.lock() {
                        Ok(mut subscriptions) => apply_event_batch(&mut subscriptions, batch),
                        Err(_) => {
                            // Never emit a route wake unless its payload was
                            // successfully appended. A poisoned store cannot
                            // uphold that contract, so drop the entire batch.
                            log::error!(
                                "[nipworker-native] subscription store poisoned; dropping event batch (messages={}, bytes={})",
                                batch_len,
                                batch_bytes
                            );
                            BatchOutcome {
                                actions: Vec::new(),
                                unsubscribes: Vec::new(),
                                dropped_events: batch_len,
                                dropped_bytes: batch_bytes,
                                missing_events: 0,
                                missing_bytes: 0,
                            }
                        }
                    };
                    let wake_count = outcome
                        .actions
                        .iter()
                        .filter(|action| matches!(action, CallbackAction::Wake(_)))
                        .count();
                    let passthrough_count = outcome
                        .actions
                        .iter()
                        .filter(|action| matches!(action, CallbackAction::Payload(_)))
                        .count();
                    log::debug!(
                        "[nipworker-native] dispatching event batch (messages={}, wakes={}, passthrough={}, droppedEvents={}, droppedBytes={}, missingEvents={}, missingBytes={})",
                        batch_len,
                        wake_count,
                        passthrough_count,
                        outcome.dropped_events,
                        outcome.dropped_bytes,
                        outcome.missing_events,
                        outcome.missing_bytes
                    );
                    for sub_id in outcome.unsubscribes {
                        let _ = callback_cmd_tx.send(EngineCommand::HandleMessage(
                            build_unsubscribe_message(&sub_id),
                        ));
                    }
                    for action in outcome.actions {
                        if !callback_alive.load(std::sync::atomic::Ordering::Acquire) {
                            break;
                        }
                        let callback_bytes = match action {
                            CallbackAction::Payload(bytes) => bytes,
                            CallbackAction::Wake(sub_id) => build_route_wake_frame(&sub_id),
                        };
                        let len = callback_bytes.len();
                        let ptr = Box::into_raw(callback_bytes.into_boxed_slice()) as *const u8;
                        callback(userdata as *mut c_void, ptr, len);
                    }
                }
            });

            // Process commands asynchronously so the LocalSet isn't blocked
            while let Some(cmd) = cmd_rx.recv().await {
                match cmd {
                    EngineCommand::HandleMessage(bytes) => {
                        // handle_message only validates and enqueues. Await it
                        // inline so a following ClearSigner cannot overtake an
                        // already accepted SetSigner command.
                        if let Err(e) = engine.handle_message(&bytes).await {
                            log::warn!("[nipworker-native] handle_message error: {}", e);
                        }
                    }
                    EngineCommand::ClearSigner => {
                        engine.clear_signer();
                    }
                    EngineCommand::RemoveSigner => {
                        engine.remove_signer();
                    }
                    EngineCommand::Wake => {
                        engine.wake();
                    }
                    EngineCommand::Shutdown => break,
                }
            }
        });

        rt.block_on(local);
    })
        .expect("failed to spawn nipworker engine thread");

    let handle = Box::new(NipworkerHandle {
        state: Mutex::new(NipworkerState {
            destroyed: false,
            alive,
            cmd_tx: Some(cmd_tx),
            subscriptions,
            mesh_tx,
            engine_thread: Some(engine_thread),
        }),
    });
    Box::into_raw(handle) as *mut c_void
}

/// Acquire the one process-wide native engine and register a delivery client.
/// The first caller's storage/relay/mesh configuration owns engine creation;
/// later callers share the exact handle, subscription store, and worker set.
/// Every successful acquire must be paired with nipworker_shared_release using
/// the same callback and userdata values.
#[no_mangle]
pub extern "C" fn nipworker_shared_acquire(
    callback: NativeCallback,
    userdata: *mut c_void,
    storage_path: *const c_char,
    default_relays: *const c_char,
    indexer_relays: *const c_char,
    mesh_enabled: bool,
) -> *mut c_void {
    let Ok(_lifecycle) = SHARED_ENGINE_LIFECYCLE.lock() else {
        return std::ptr::null_mut();
    };
    let callback_address = callback as usize;
    let userdata_address = userdata as usize;
    let (mutex, _) = shared_engine();
    let Ok(mut state) = mutex.lock() else {
        return std::ptr::null_mut();
    };
    if state.handle != 0 {
        let client_references = if let Some(client) = state.clients.iter_mut().find(|client| {
            client.callback as usize == callback_address && client.userdata == userdata_address
        }) {
            client.references += 1;
            client.references
        } else {
            state.clients.push(SharedClient {
                callback,
                userdata: userdata_address,
                references: 1,
            });
            1
        };
        log::info!(
            "[nipworker-native] shared engine acquire action=reuse generation={} handle=0x{:x} client=0x{:x} callback=0x{:x} clientReferences={} clients={} totalReferences={}",
            state.generation,
            state.handle,
            userdata_address,
            callback_address,
            client_references,
            state.clients.len(),
            shared_reference_count(&state)
        );
        return state.handle as *mut c_void;
    }

    let generation = state.generation.wrapping_add(1).max(1);
    state.clients.push(SharedClient {
        callback,
        userdata: userdata_address,
        references: 1,
    });
    log::info!(
        "[nipworker-native] shared engine acquire action=create-start generation={} client=0x{:x} callback=0x{:x} clients={} totalReferences={}",
        generation,
        userdata_address,
        callback_address,
        state.clients.len(),
        shared_reference_count(&state)
    );
    drop(state);

    let handle = nipworker_init_with_options(
        shared_engine_callback,
        std::ptr::null_mut(),
        storage_path,
        default_relays,
        indexer_relays,
        mesh_enabled,
    );
    let Ok(mut state) = mutex.lock() else {
        if !handle.is_null() {
            nipworker_deinit(handle);
        }
        return std::ptr::null_mut();
    };
    if handle.is_null() {
        state.clients.retain(|client| {
            client.callback as usize != callback_address || client.userdata != userdata_address
        });
        log::error!(
            "[nipworker-native] shared engine acquire action=create-failed generation={} client=0x{:x} callback=0x{:x} clients={} totalReferences={}",
            generation,
            userdata_address,
            callback_address,
            state.clients.len(),
            shared_reference_count(&state)
        );
        return std::ptr::null_mut();
    }
    state.handle = handle as usize;
    state.generation = generation;
    log::info!(
        "[nipworker-native] shared engine acquire action=created generation={} handle=0x{:x} client=0x{:x} callback=0x{:x} clients={} totalReferences={}",
        state.generation,
        state.handle,
        userdata_address,
        callback_address,
        state.clients.len(),
        shared_reference_count(&state)
    );
    #[cfg(test)]
    SHARED_ENGINE_INITIALIZATIONS.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    handle
}

/// Pin the current shared engine to the native process lifetime. This anchor
/// is idempotent rather than reference counted: React Native runtimes and
/// pure-native facades may all request it, while only an explicit application
/// shutdown or test reset should release it. Call shared_acquire first when a
/// delivery client is also needed so startup packets have a consumer.
#[no_mangle]
pub extern "C" fn nipworker_shared_process_acquire(
    storage_path: *const c_char,
    default_relays: *const c_char,
    indexer_relays: *const c_char,
    mesh_enabled: bool,
) -> *mut c_void {
    let Ok(_lifecycle) = SHARED_ENGINE_LIFECYCLE.lock() else {
        return std::ptr::null_mut();
    };
    let (mutex, _) = shared_engine();
    let Ok(mut state) = mutex.lock() else {
        return std::ptr::null_mut();
    };
    if state.handle != 0 {
        state.process_anchored = true;
        log::info!(
            "[nipworker-native] shared engine anchor action=retain generation={} handle=0x{:x} clients={} totalReferences={}",
            state.generation,
            state.handle,
            state.clients.len(),
            shared_reference_count(&state)
        );
        return state.handle as *mut c_void;
    }

    let generation = state.generation.wrapping_add(1).max(1);
    state.process_anchored = true;
    log::info!(
        "[nipworker-native] shared engine anchor action=create-start generation={} clients=0 totalReferences=0",
        generation
    );
    drop(state);

    let handle = nipworker_init_with_options(
        shared_engine_callback,
        std::ptr::null_mut(),
        storage_path,
        default_relays,
        indexer_relays,
        mesh_enabled,
    );
    let Ok(mut state) = mutex.lock() else {
        if !handle.is_null() {
            nipworker_deinit(handle);
        }
        return std::ptr::null_mut();
    };
    if handle.is_null() {
        state.process_anchored = false;
        log::error!(
            "[nipworker-native] shared engine anchor action=create-failed generation={}",
            generation
        );
        return std::ptr::null_mut();
    }
    state.handle = handle as usize;
    state.generation = generation;
    log::info!(
        "[nipworker-native] shared engine anchor action=created generation={} handle=0x{:x} clients=0 totalReferences=0",
        state.generation,
        state.handle
    );
    #[cfg(test)]
    SHARED_ENGINE_INITIALIZATIONS.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    handle
}

/// Remove the process-lifetime pin. The engine remains alive while delivery
/// clients exist and is synchronously joined once the last client is gone.
/// Normal RN runtime invalidation must not call this function.
#[no_mangle]
pub extern "C" fn nipworker_shared_process_release() {
    let Ok(_lifecycle) = SHARED_ENGINE_LIFECYCLE.lock() else {
        return;
    };
    let (mutex, changed) = shared_engine();
    let Ok(mut state) = mutex.lock() else {
        return;
    };
    if !state.process_anchored {
        return;
    }
    state.process_anchored = false;
    if !state.clients.is_empty() {
        log::info!(
            "[nipworker-native] shared engine anchor action=release generation={} handle=0x{:x} clients={} totalReferences={}",
            state.generation,
            state.handle,
            state.clients.len(),
            shared_reference_count(&state)
        );
        return;
    }
    while state.callbacks_in_flight != 0 {
        let Ok(next) = changed.wait(state) else {
            return;
        };
        state = next;
    }
    let handle = state.handle as *mut c_void;
    let generation = state.generation;
    state.handle = 0;
    log::info!(
        "[nipworker-native] shared engine anchor action=final-release generation={} handle=0x{:x} clients=0 totalReferences=0",
        generation,
        handle as usize
    );
    drop(state);
    if !handle.is_null() {
        nipworker_deinit(handle);
        log::info!(
            "[nipworker-native] shared engine lifecycle action=deinitialized generation={} handle=0x{:x}",
            generation,
            handle as usize
        );
    }
}

/// Release one client reference. The final release synchronously joins an
/// unanchored shared engine; a process anchor instead keeps it available for
/// the next runtime client. Removal waits for every callback snapshot that
/// could still reference userdata, so wrappers may reclaim their callback
/// context on return. Calling this synchronously from a callback is unsupported.
#[no_mangle]
pub extern "C" fn nipworker_shared_release(
    handle: *mut c_void,
    callback: NativeCallback,
    userdata: *mut c_void,
) {
    if handle.is_null() {
        return;
    }
    let Ok(_lifecycle) = SHARED_ENGINE_LIFECYCLE.lock() else {
        return;
    };
    let callback_address = callback as usize;
    let userdata_address = userdata as usize;
    let (mutex, changed) = shared_engine();
    let Ok(mut state) = mutex.lock() else {
        return;
    };
    if state.handle != handle as usize {
        log::debug!(
            "[nipworker-native] shared engine release action=ignored-stale generation={} requestedHandle=0x{:x} activeHandle=0x{:x} client=0x{:x} callback=0x{:x}",
            state.generation,
            handle as usize,
            state.handle,
            userdata_address,
            callback_address
        );
        return;
    }
    let Some(index) = state.clients.iter().position(|client| {
        client.callback as usize == callback_address && client.userdata == userdata_address
    }) else {
        log::debug!(
            "[nipworker-native] shared engine release action=ignored-client generation={} handle=0x{:x} client=0x{:x} callback=0x{:x}",
            state.generation,
            state.handle,
            userdata_address,
            callback_address
        );
        return;
    };
    if state.clients[index].references > 1 {
        state.clients[index].references -= 1;
        log::info!(
            "[nipworker-native] shared engine release action=reference generation={} handle=0x{:x} client=0x{:x} callback=0x{:x} clientReferences={} clients={} totalReferences={}",
            state.generation,
            state.handle,
            userdata_address,
            callback_address,
            state.clients[index].references,
            state.clients.len(),
            shared_reference_count(&state)
        );
        return;
    }
    state.clients.remove(index);
    while state.callbacks_in_flight != 0 {
        let Ok(next) = changed.wait(state) else {
            return;
        };
        state = next;
    }
    if !state.clients.is_empty() {
        log::info!(
            "[nipworker-native] shared engine release action=client generation={} handle=0x{:x} client=0x{:x} callback=0x{:x} clients={} totalReferences={}",
            state.generation,
            state.handle,
            userdata_address,
            callback_address,
            state.clients.len(),
            shared_reference_count(&state)
        );
        return;
    }
    if state.process_anchored {
        log::info!(
            "[nipworker-native] shared engine release action=last-client-anchored generation={} handle=0x{:x} client=0x{:x} callback=0x{:x} clients=0 totalReferences=0",
            state.generation,
            state.handle,
            userdata_address,
            callback_address
        );
        return;
    }
    let generation = state.generation;
    log::info!(
        "[nipworker-native] shared engine release action=final generation={} handle=0x{:x} client=0x{:x} callback=0x{:x} clients=0 totalReferences=0",
        generation,
        state.handle,
        userdata_address,
        callback_address
    );
    state.handle = 0;
    drop(state);
    nipworker_deinit(handle);
    log::info!(
        "[nipworker-native] shared engine lifecycle action=deinitialized generation={} handle=0x{:x}",
        generation,
        handle as usize
    );
}

#[no_mangle]
pub unsafe extern "C" fn nipworker_wake(handle: *mut c_void) {
    if handle.is_null() {
        return;
    }
    let handle = unsafe { &*(handle as *mut NipworkerHandle) };
    if let Ok(state) = handle.state.lock() {
        if state.destroyed {
            return;
        }
        if let Some(ref tx) = state.cmd_tx {
            let _ = tx.send(EngineCommand::Wake);
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn nipworker_handle_message(handle: *mut c_void, ptr: *const u8, len: usize) {
    if handle.is_null() || ptr.is_null() {
        return;
    }
    let handle = unsafe { &*(handle as *mut NipworkerHandle) };
    let bytes = unsafe { slice::from_raw_parts(ptr, len) }.to_vec();
    if let Ok(state) = handle.state.lock() {
        if state.destroyed {
            return;
        }
        if let Some(ref tx) = state.cmd_tx {
            let _ = tx.send(EngineCommand::HandleMessage(bytes));
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn nipworker_subscribe_message(
    handle: *mut c_void,
    ptr: *const u8,
    len: usize,
) -> bool {
    if handle.is_null() || ptr.is_null() {
        return false;
    }
    let handle_ref = unsafe { &*(handle as *mut NipworkerHandle) };
    let bytes = unsafe { slice::from_raw_parts(ptr, len) }.to_vec();
    let Some((sub_id, buffer_size)) = subscription_buffer_size_from_message(&bytes) else {
        return false;
    };
    if let Ok(state) = handle_ref.state.lock() {
        if state.destroyed {
            return false;
        }
        if let Ok(mut subscriptions) = state.subscriptions.lock() {
            subscriptions.register(sub_id, buffer_size, true);
        } else {
            return false;
        }
        if let Some(ref tx) = state.cmd_tx {
            return tx.send(EngineCommand::HandleMessage(bytes)).is_ok();
        }
    }
    false
}

#[no_mangle]
pub unsafe extern "C" fn nipworker_publish_message(
    handle: *mut c_void,
    ptr: *const u8,
    len: usize,
) -> bool {
    if handle.is_null() || ptr.is_null() {
        return false;
    }
    let handle_ref = unsafe { &*(handle as *mut NipworkerHandle) };
    let bytes = unsafe { slice::from_raw_parts(ptr, len) }.to_vec();
    let Some(publish_id) = publish_id_from_message(&bytes) else {
        return false;
    };
    if let Ok(state) = handle_ref.state.lock() {
        if state.destroyed {
            return false;
        }
        if let Ok(mut subscriptions) = state.subscriptions.lock() {
            subscriptions.register(publish_id, 3072, false);
        } else {
            return false;
        }
        if let Some(ref tx) = state.cmd_tx {
            return tx.send(EngineCommand::HandleMessage(bytes)).is_ok();
        }
    }
    false
}

#[no_mangle]
pub unsafe extern "C" fn nipworker_set_private_key(handle: *mut c_void, ptr: *const c_char) {
    if handle.is_null() || ptr.is_null() {
        return;
    }
    let handle = unsafe { &*(handle as *mut NipworkerHandle) };
    let secret = unsafe { CStr::from_ptr(ptr) }.to_string_lossy().to_string();
    if let Ok(state) = handle.state.lock() {
        if state.destroyed {
            return;
        }
        if let Some(ref tx) = state.cmd_tx {
            let bytes = build_set_private_key_message(&secret);
            let _ = tx.send(EngineCommand::HandleMessage(bytes));
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn nipworker_clear_signer(handle: *mut c_void) {
    if handle.is_null() {
        return;
    }
    let handle = unsafe { &*(handle as *mut NipworkerHandle) };
    if let Ok(state) = handle.state.lock() {
        if state.destroyed {
            return;
        }
        if let Some(ref tx) = state.cmd_tx {
            let _ = tx.send(EngineCommand::ClearSigner);
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn nipworker_remove_signer(handle: *mut c_void) {
    if handle.is_null() {
        return;
    }
    let handle = unsafe { &*(handle as *mut NipworkerHandle) };
    if let Ok(state) = handle.state.lock() {
        if state.destroyed {
            return;
        }
        if let Some(ref tx) = state.cmd_tx {
            let _ = tx.send(EngineCommand::RemoveSigner);
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn nipworker_register_subscription(
    handle: *mut c_void,
    sub_id: *const c_char,
    buffer_size: usize,
) -> bool {
    if handle.is_null() || sub_id.is_null() {
        return false;
    }
    let handle = unsafe { &*(handle as *mut NipworkerHandle) };
    let sub_id = unsafe { CStr::from_ptr(sub_id) }
        .to_string_lossy()
        .to_string();
    if let Ok(state) = handle.state.lock() {
        if state.destroyed {
            return false;
        }
        if let Ok(mut subscriptions) = state.subscriptions.lock() {
            subscriptions.register(sub_id, buffer_size, true);
            return true;
        }
    }
    false
}

#[no_mangle]
pub unsafe extern "C" fn nipworker_register_publish_buffer(
    handle: *mut c_void,
    publish_id: *const c_char,
    buffer_size: usize,
) -> bool {
    if handle.is_null() || publish_id.is_null() {
        return false;
    }
    let handle = unsafe { &*(handle as *mut NipworkerHandle) };
    let publish_id = unsafe { CStr::from_ptr(publish_id) }
        .to_string_lossy()
        .to_string();
    if let Ok(state) = handle.state.lock() {
        if state.destroyed {
            return false;
        }
        if let Ok(mut subscriptions) = state.subscriptions.lock() {
            subscriptions.register(publish_id, buffer_size, false);
            return true;
        }
    }
    false
}

#[no_mangle]
pub unsafe extern "C" fn nipworker_retain_subscription(
    handle: *mut c_void,
    sub_id: *const c_char,
) -> bool {
    if handle.is_null() || sub_id.is_null() {
        return false;
    }
    let handle = unsafe { &*(handle as *mut NipworkerHandle) };
    let sub_id = unsafe { CStr::from_ptr(sub_id) }.to_string_lossy();
    if let Ok(state) = handle.state.lock() {
        if state.destroyed {
            return false;
        }
        if let Ok(mut subscriptions) = state.subscriptions.lock() {
            return subscriptions.retain(&sub_id);
        }
    }
    false
}

#[no_mangle]
pub unsafe extern "C" fn nipworker_release_subscription(
    handle: *mut c_void,
    sub_id: *const c_char,
) {
    if handle.is_null() || sub_id.is_null() {
        return;
    }
    let handle = unsafe { &*(handle as *mut NipworkerHandle) };
    let sub_id = unsafe { CStr::from_ptr(sub_id) }.to_string_lossy();
    if let Ok(state) = handle.state.lock() {
        if state.destroyed {
            return;
        }
        if let Ok(mut subscriptions) = state.subscriptions.lock() {
            subscriptions.release(&sub_id);
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn nipworker_subscription_buffer_ptr(
    handle: *mut c_void,
    sub_id: *const c_char,
) -> *mut u8 {
    if handle.is_null() || sub_id.is_null() {
        return std::ptr::null_mut();
    }
    let handle = unsafe { &*(handle as *mut NipworkerHandle) };
    let sub_id = unsafe { CStr::from_ptr(sub_id) }.to_string_lossy();
    if let Ok(state) = handle.state.lock() {
        if state.destroyed {
            return std::ptr::null_mut();
        }
        if let Ok(mut subscriptions) = state.subscriptions.lock() {
            if let Some(subscription) = subscriptions.subscriptions.get_mut(sub_id.as_ref()) {
                return subscription.backing.as_mut_ptr();
            }
        }
    }
    std::ptr::null_mut()
}

#[no_mangle]
pub unsafe extern "C" fn nipworker_subscription_buffer_len(
    handle: *mut c_void,
    sub_id: *const c_char,
) -> usize {
    if handle.is_null() || sub_id.is_null() {
        return 0;
    }
    let handle = unsafe { &*(handle as *mut NipworkerHandle) };
    let sub_id = unsafe { CStr::from_ptr(sub_id) }.to_string_lossy();
    if let Ok(state) = handle.state.lock() {
        if state.destroyed {
            return 0;
        }
        if let Ok(subscriptions) = state.subscriptions.lock() {
            if let Some(subscription) = subscriptions.subscriptions.get(sub_id.as_ref()) {
                return subscription.backing.len();
            }
        }
    }
    0
}

/// Atomically pin one subscription buffer's fixed allocation and return an
/// opaque lifetime token. This memory pin is independent of the subscription's
/// logical ref_count: cleanup may remove/unsubscribe the route while the token
/// keeps only its bytes valid across runtime teardown.
#[no_mangle]
pub unsafe extern "C" fn nipworker_subscription_pin(
    handle: *mut c_void,
    sub_id: *const c_char,
    out_data: *mut *mut u8,
    out_len: *mut usize,
) -> *mut c_void {
    if !out_data.is_null() {
        unsafe { *out_data = std::ptr::null_mut() };
    }
    if !out_len.is_null() {
        unsafe { *out_len = 0 };
    }
    if handle.is_null() || sub_id.is_null() || out_data.is_null() || out_len.is_null() {
        return std::ptr::null_mut();
    }

    let handle = unsafe { &*(handle as *mut NipworkerHandle) };
    let sub_id = unsafe { CStr::from_ptr(sub_id) }.to_string_lossy();
    let subscriptions = match handle.state.lock() {
        Ok(state) if !state.destroyed => state.subscriptions.clone(),
        _ => return std::ptr::null_mut(),
    };

    let backing = match subscriptions.lock() {
        Ok(store) => {
            let Some(subscription) = store.subscriptions.get(sub_id.as_ref()) else {
                return std::ptr::null_mut();
            };
            subscription.backing.clone()
        }
        Err(_) => return std::ptr::null_mut(),
    };

    unsafe {
        *out_data = backing.as_mut_ptr();
        *out_len = backing.len();
    }
    Box::into_raw(Box::new(NipworkerSubscriptionPin { _backing: backing })) as *mut c_void
}

/// Release a token returned by `nipworker_subscription_pin`. This intentionally
/// does not consult the engine handle: JSI ArrayBuffers may be finalized after
/// their originating runtime and engine generation have been destroyed.
#[no_mangle]
pub unsafe extern "C" fn nipworker_subscription_pin_release(pin: *mut c_void) {
    if pin.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(pin as *mut NipworkerSubscriptionPin) });
}

/// Reset a subscription buffer's write cursor back to 4 so drained space can be
/// reused. The host must call this only after fully draining the buffer up to
/// `expected_write_pos`. Returns false when the sub is missing or the engine
/// appended since the host last read (cursor != expected_write_pos) — in that
/// case the host should re-read the buffer instead of resetting.
#[no_mangle]
pub unsafe extern "C" fn nipworker_subscription_try_reset(
    handle: *mut c_void,
    sub_id: *const c_char,
    expected_write_pos: u32,
) -> bool {
    if handle.is_null() || sub_id.is_null() {
        return false;
    }
    let handle = unsafe { &*(handle as *mut NipworkerHandle) };
    let sub_id = unsafe { CStr::from_ptr(sub_id) }.to_string_lossy();
    if let Ok(state) = handle.state.lock() {
        if state.destroyed {
            return false;
        }
        if let Ok(mut subscriptions) = state.subscriptions.lock() {
            if let Some(subscription) = subscriptions.subscriptions.get_mut(sub_id.as_ref()) {
                return subscription.try_reset(expected_write_pos);
            }
        }
    }
    false
}

#[no_mangle]
pub unsafe extern "C" fn nipworker_cleanup_subscriptions(handle: *mut c_void) {
    if handle.is_null() {
        return;
    }
    let handle = unsafe { &*(handle as *mut NipworkerHandle) };
    let mut to_delete = Vec::new();
    let tx = if let Ok(state) = handle.state.lock() {
        if state.destroyed {
            return;
        }
        if let Ok(subscriptions) = state.subscriptions.lock() {
            for (sub_id, subscription) in subscriptions.subscriptions.iter() {
                if subscription.ref_count <= 0
                    && sub_id != "notifications"
                    && sub_id != "starterpack"
                {
                    to_delete.push((sub_id.clone(), subscription.close_on_cleanup));
                }
            }
        }
        state.cmd_tx.clone()
    } else {
        None
    };

    if to_delete.is_empty() {
        return;
    }

    let mut removed = Vec::new();
    if let Ok(state) = handle.state.lock() {
        if let Ok(mut subscriptions) = state.subscriptions.lock() {
            for (sub_id, close_on_cleanup) in &to_delete {
                let still_releasable = subscriptions
                    .subscriptions
                    .get(sub_id)
                    .is_some_and(|subscription| subscription.ref_count <= 0);
                if still_releasable {
                    subscriptions.subscriptions.remove(sub_id);
                    removed.push((sub_id.clone(), *close_on_cleanup));
                }
            }
        }
    }

    if let Some(tx) = tx {
        for (sub_id, close_on_cleanup) in removed {
            if close_on_cleanup {
                let _ = tx.send(EngineCommand::HandleMessage(build_unsubscribe_message(
                    &sub_id,
                )));
            }
        }
    }
}

/// Free an owned buffer previously passed to the callback in
/// `nipworker_init`. The host may adopt the allocation without copying, but
/// must call this when its native queue has finished with the packet.
#[no_mangle]
pub unsafe extern "C" fn nipworker_free_bytes(ptr: *mut u8, len: usize) {
    if !ptr.is_null() && len > 0 {
        let _ = unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)) };
    }
}

#[no_mangle]
pub extern "C" fn nipworker_deinit(handle: *mut c_void) {
    if handle.is_null() {
        return;
    }
    let handle = unsafe { &*(handle as *mut NipworkerHandle) };
    let engine_thread = if let Ok(mut state) = handle.state.lock() {
        if state.destroyed {
            return;
        }
        state.destroyed = true;
        state
            .alive
            .store(false, std::sync::atomic::Ordering::Release);
        state.mesh_tx.take();
        if let Some(cmd_tx) = state.cmd_tx.take() {
            let _ = cmd_tx.send(EngineCommand::Shutdown);
        }
        state.engine_thread.take()
    } else {
        None
    };

    if let Some(engine_thread) = engine_thread {
        if engine_thread.thread().id() == thread::current().id() {
            // The callback contract forbids synchronous teardown from the
            // callback itself. Avoid a self-join deadlock if a foreign host
            // violates that contract; the sent Shutdown still tears it down.
            log::error!("[nipworker-native] deinit called synchronously from engine callback");
        } else if engine_thread.join().is_err() {
            log::error!("[nipworker-native] engine thread panicked during deinit");
        }
    }

    // The callback task is gone after the join, so the state no longer needs
    // to retain any subscription allocation. Outstanding opaque pin tokens
    // keep the old store alive until their JSI ArrayBuffers are finalized.
    if let Ok(mut state) = handle.state.lock() {
        state.subscriptions = Arc::new(Mutex::new(NativeSubscriptionStore::new()));
    }

    // Keep only the small handle tombstone so concurrent legacy C ABI calls can
    // observe `destroyed` instead of dereferencing freed memory. Engine/runtime,
    // network workers, callback task, mesh runtime and their OS threads are all
    // joined above. Subscription allocations live only while an opaque pin token
    // owns them; the new JSI adapter does not retain this handle.
}

#[cfg(test)]
mod tests {
    use super::{
        apply_event_batch, nipworker_cleanup_subscriptions, nipworker_clear_signer,
        nipworker_deinit, nipworker_free_bytes, nipworker_init, nipworker_register_subscription,
        nipworker_release_subscription, nipworker_remove_signer, nipworker_set_private_key,
        nipworker_shared_acquire, nipworker_shared_process_acquire,
        nipworker_shared_process_release, nipworker_shared_release, nipworker_subscription_pin,
        nipworker_subscription_pin_release, nipworker_subscription_try_reset,
        parse_native_log_level, shared_engine, AppendOutcome, BatchOutcome, CallbackAction,
        EngineCommand, NativeSubscription, NativeSubscriptionStore, NipworkerHandle,
        NipworkerState, LOG_LEVEL_DEBUG, LOG_LEVEL_ERROR, LOG_LEVEL_INFO, LOG_LEVEL_TRACE,
        LOG_LEVEL_WARN, SHARED_ENGINE_INITIALIZATIONS,
    };
    use std::ffi::{c_void, CString};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    extern "C" fn counting_callback(userdata: *mut c_void, ptr: *const u8, len: usize) {
        let callbacks = unsafe { &*(userdata as *const AtomicUsize) };
        callbacks.fetch_add(1, Ordering::AcqRel);
        unsafe { nipworker_free_bytes(ptr as *mut u8, len) };
    }

    #[test]
    fn native_log_level_parser_is_case_insensitive_and_secure_by_default() {
        assert_eq!(parse_native_log_level("trace"), LOG_LEVEL_TRACE);
        assert_eq!(parse_native_log_level("DEBUG"), LOG_LEVEL_DEBUG);
        assert_eq!(parse_native_log_level(" info "), LOG_LEVEL_INFO);
        assert_eq!(parse_native_log_level("warn"), LOG_LEVEL_WARN);
        assert_eq!(parse_native_log_level("error"), LOG_LEVEL_ERROR);
        assert_eq!(parse_native_log_level("invalid"), LOG_LEVEL_ERROR);
    }

    fn wake_ids(outcome: &BatchOutcome) -> Vec<String> {
        outcome
            .actions
            .iter()
            .filter_map(|action| match action {
                CallbackAction::Wake(sub_id) => Some(sub_id.clone()),
                _ => None,
            })
            .collect()
    }

    fn payloads(outcome: &BatchOutcome) -> Vec<Vec<u8>> {
        outcome
            .actions
            .iter()
            .filter_map(|action| match action {
                CallbackAction::Payload(bytes) => Some(bytes.clone()),
                _ => None,
            })
            .collect()
    }

    fn test_handle() -> *mut c_void {
        let handle = Box::new(NipworkerHandle {
            state: Mutex::new(NipworkerState {
                destroyed: false,
                alive: Arc::new(std::sync::atomic::AtomicBool::new(true)),
                cmd_tx: None,
                subscriptions: Arc::new(Mutex::new(NativeSubscriptionStore::new())),
                mesh_tx: None,
                engine_thread: None,
            }),
        });
        Box::into_raw(handle) as *mut c_void
    }

    #[test]
    fn new_subscription_initializes_header_to_four() {
        let subscription = NativeSubscription::new(64, false);
        assert_eq!(subscription.bytes().len(), 64);
        assert_eq!(&subscription.bytes()[0..4], &4u32.to_le_bytes());
    }

    #[test]
    fn clear_signer_enqueues_native_engine_command() {
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel();
        let handle = Box::new(NipworkerHandle {
            state: Mutex::new(NipworkerState {
                destroyed: false,
                alive: Arc::new(std::sync::atomic::AtomicBool::new(true)),
                cmd_tx: Some(cmd_tx),
                subscriptions: Arc::new(Mutex::new(NativeSubscriptionStore::new())),
                mesh_tx: None,
                engine_thread: None,
            }),
        });
        let handle = Box::into_raw(handle) as *mut c_void;

        unsafe { nipworker_clear_signer(handle) };

        assert!(matches!(cmd_rx.try_recv(), Ok(EngineCommand::ClearSigner)));
        let _ = unsafe { Box::from_raw(handle as *mut NipworkerHandle) };
    }

    #[test]
    fn remove_signer_enqueues_native_engine_command() {
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel();
        let handle = Box::new(NipworkerHandle {
            state: Mutex::new(NipworkerState {
                destroyed: false,
                alive: Arc::new(std::sync::atomic::AtomicBool::new(true)),
                cmd_tx: Some(cmd_tx),
                subscriptions: Arc::new(Mutex::new(NativeSubscriptionStore::new())),
                mesh_tx: None,
                engine_thread: None,
            }),
        });
        let handle = Box::into_raw(handle) as *mut c_void;

        unsafe { nipworker_remove_signer(handle) };

        assert!(matches!(cmd_rx.try_recv(), Ok(EngineCommand::RemoveSigner)));
        let _ = unsafe { Box::from_raw(handle as *mut NipworkerHandle) };
    }

    #[test]
    fn append_payload_records_length_and_advances_cursor() {
        let mut subscription = NativeSubscription::new(64, false);
        let payload = b"\x0a\x0b\x0c\x0d";

        let written = subscription.append_payload(payload);
        assert!(written, "payload should fit");
        assert_eq!(
            &subscription.bytes()[4..8],
            &(payload.len() as u32).to_le_bytes()
        );
        assert_eq!(&subscription.bytes()[8..12], payload);
        assert_eq!(&subscription.bytes()[0..4], &(12u32).to_le_bytes());
    }

    #[test]
    fn append_payload_rejects_overflow_without_panic() {
        let mut subscription = NativeSubscription::new(12, false);
        let large = vec![0u8; 16];

        let written = subscription.append_payload(&large);
        assert!(
            !written,
            "payload larger than remaining space should be rejected"
        );
        assert_eq!(&subscription.bytes()[0..4], &4u32.to_le_bytes());
    }

    #[test]
    fn register_reuses_existing_subscription_without_reset() {
        let mut store = NativeSubscriptionStore::new();
        store.register("sub-1".to_string(), 32, false);
        assert_eq!(store.subscriptions.get("sub-1").unwrap().ref_count, 1);

        let first = store.subscriptions.get("sub-1").unwrap().bytes()[0];
        store.register("sub-1".to_string(), 32, false);
        let second = store.subscriptions.get("sub-1").unwrap();

        assert_eq!(second.ref_count, 2);
        assert_eq!(second.bytes()[0], first);
    }

    #[test]
    fn overflow_keeps_subscription_buffer_for_reader_consistency() {
        let mut store = NativeSubscriptionStore::new();
        store.register("sub-1".to_string(), 12, true);
        let large = vec![0u8; 16];

        let result = store.append_payload("sub-1", &large);

        assert!(matches!(
            result,
            AppendOutcome::Full {
                close_on_cleanup: true
            }
        ));
        assert!(store.subscriptions.contains_key("sub-1"));
        assert_eq!(
            &store.subscriptions.get("sub-1").unwrap().bytes()[0..4],
            &4u32.to_le_bytes()
        );
    }

    #[test]
    fn batch_coalesces_multiple_messages_for_same_sub_into_one_wake() {
        let mut store = NativeSubscriptionStore::new();
        store.register("sub-1".to_string(), 64, true);
        let batch = vec![
            ("sub-1".to_string(), b"aa".to_vec()),
            ("sub-1".to_string(), b"bb".to_vec()),
            ("sub-1".to_string(), b"cc".to_vec()),
        ];

        let outcome = apply_event_batch(&mut store, batch);

        assert_eq!(wake_ids(&outcome), vec!["sub-1".to_string()]);
        assert!(payloads(&outcome).is_empty());
        assert!(outcome.unsubscribes.is_empty());
    }

    #[test]
    fn batch_emits_one_wake_per_sub_across_subs() {
        let mut store = NativeSubscriptionStore::new();
        store.register("sub-a".to_string(), 64, true);
        store.register("sub-b".to_string(), 64, true);
        let batch = vec![
            ("sub-a".to_string(), b"1".to_vec()),
            ("sub-b".to_string(), b"2".to_vec()),
            ("sub-a".to_string(), b"3".to_vec()),
            ("sub-b".to_string(), b"4".to_vec()),
        ];

        let outcome = apply_event_batch(&mut store, batch);

        let wakes = wake_ids(&outcome);
        assert_eq!(wakes.len(), 2);
        assert!(wakes.contains(&"sub-a".to_string()));
        assert!(wakes.contains(&"sub-b".to_string()));
        assert_eq!(outcome.actions.len(), 2);
    }

    #[test]
    fn batch_passes_crypto_and_empty_sub_messages_through_uncoalesced() {
        let mut store = NativeSubscriptionStore::new();
        store.register("sub-1".to_string(), 64, true);
        let batch = vec![
            ("".to_string(), b"x".to_vec()),
            ("crypto".to_string(), b"y".to_vec()),
            ("sub-1".to_string(), b"z".to_vec()),
            ("".to_string(), b"w".to_vec()),
        ];

        let outcome = apply_event_batch(&mut store, batch);

        assert_eq!(
            payloads(&outcome),
            vec![b"x".to_vec(), b"y".to_vec(), b"w".to_vec()]
        );
        assert_eq!(wake_ids(&outcome), vec!["sub-1".to_string()]);
        assert_eq!(outcome.actions.len(), 4);
        // Passthrough payloads are not appended to any subscription buffer.
        assert_eq!(
            &store.subscriptions.get("sub-1").unwrap().bytes()[0..4],
            &(9u32).to_le_bytes()
        );
    }

    #[test]
    fn batch_buffer_full_triggers_unsubscribe_without_wake() {
        let mut store = NativeSubscriptionStore::new();
        store.register("sub-full".to_string(), 8, true);
        store.register("sub-full-keep".to_string(), 8, false);
        let batch = vec![
            ("sub-full".to_string(), vec![0u8; 16]),
            ("sub-full-keep".to_string(), vec![0u8; 16]),
        ];

        let outcome = apply_event_batch(&mut store, batch);

        assert_eq!(outcome.unsubscribes, vec!["sub-full".to_string()]);
        assert!(wake_ids(&outcome).is_empty());
        assert!(payloads(&outcome).is_empty());
        assert_eq!(outcome.dropped_events, 2);
        assert_eq!(outcome.dropped_bytes, 32);
        assert_eq!(outcome.missing_events, 0);
        assert_eq!(outcome.missing_bytes, 0);
    }

    #[test]
    fn batch_deduplicates_overflow_unsubscribe_for_same_sub() {
        let mut store = NativeSubscriptionStore::new();
        store.register("sub-full".to_string(), 8, true);
        let batch = vec![
            ("sub-full".to_string(), vec![0u8; 16]),
            ("sub-full".to_string(), vec![0u8; 24]),
            ("sub-full".to_string(), vec![0u8; 32]),
        ];

        let outcome = apply_event_batch(&mut store, batch);

        assert_eq!(outcome.unsubscribes, vec!["sub-full".to_string()]);
        assert!(outcome.actions.is_empty());
        assert_eq!(outcome.dropped_events, 3);
        assert_eq!(outcome.dropped_bytes, 72);
    }

    #[test]
    fn batch_drops_missing_subscription_without_route_wake() {
        let mut store = NativeSubscriptionStore::new();
        let batch = vec![
            ("removed-sub".to_string(), vec![1u8; 11]),
            ("removed-sub".to_string(), vec![2u8; 13]),
        ];

        let outcome = apply_event_batch(&mut store, batch);

        assert!(outcome.actions.is_empty());
        assert!(outcome.unsubscribes.is_empty());
        assert_eq!(outcome.dropped_events, 2);
        assert_eq!(outcome.dropped_bytes, 24);
        assert_eq!(outcome.missing_events, 2);
        assert_eq!(outcome.missing_bytes, 24);
    }

    #[test]
    fn batch_of_ten_thousand_events_has_one_wake_and_complete_buffer() {
        const EVENT_COUNT: usize = 10_000;
        const PAYLOAD_LEN: usize = 4;
        const FRAME_LEN: usize = 4 + PAYLOAD_LEN;

        let mut store = NativeSubscriptionStore::new();
        store.register("burst-sub".to_string(), 4 + EVENT_COUNT * FRAME_LEN, true);
        let batch = (0..EVENT_COUNT)
            .map(|index| {
                (
                    "burst-sub".to_string(),
                    (index as u32).to_le_bytes().to_vec(),
                )
            })
            .collect();

        let outcome = apply_event_batch(&mut store, batch);

        assert_eq!(wake_ids(&outcome), vec!["burst-sub".to_string()]);
        assert_eq!(outcome.dropped_events, 0);
        assert!(outcome.unsubscribes.is_empty());
        let subscription = store.subscriptions.get("burst-sub").unwrap();
        assert_eq!(
            subscription.write_pos() as usize,
            4 + EVENT_COUNT * FRAME_LEN
        );
        for index in [0usize, 255, 256, EVENT_COUNT - 1] {
            let frame_start = 4 + index * FRAME_LEN;
            assert_eq!(
                &subscription.bytes()[frame_start..frame_start + 4],
                &(PAYLOAD_LEN as u32).to_le_bytes()
            );
            assert_eq!(
                &subscription.bytes()[frame_start + 4..frame_start + FRAME_LEN],
                &(index as u32).to_le_bytes()
            );
        }
    }

    #[test]
    fn batch_appends_payloads_in_channel_order() {
        let mut store = NativeSubscriptionStore::new();
        store.register("sub-1".to_string(), 64, true);
        let batch = vec![
            ("sub-1".to_string(), b"first".to_vec()),
            ("sub-1".to_string(), b"second".to_vec()),
        ];

        apply_event_batch(&mut store, batch);

        let subscription = store.subscriptions.get("sub-1").unwrap();
        let buffer = subscription.bytes();
        // 4-byte header, then [len][payload] frames in append order.
        assert_eq!(&buffer[4..8], &5u32.to_le_bytes());
        assert_eq!(&buffer[8..13], b"first");
        assert_eq!(&buffer[13..17], &6u32.to_le_bytes());
        assert_eq!(&buffer[17..23], b"second");
        assert_eq!(&buffer[0..4], &(23u32).to_le_bytes());
    }

    #[test]
    fn try_reset_succeeds_when_positions_match() {
        let handle = test_handle();
        let sub_id = CString::new("sub-1").unwrap();
        let handle_ref = unsafe { &*(handle as *mut NipworkerHandle) };
        if let Ok(state) = handle_ref.state.lock() {
            if let Ok(mut subscriptions) = state.subscriptions.lock() {
                subscriptions.register("sub-1".to_string(), 64, true);
                assert!(matches!(
                    subscriptions.append_payload("sub-1", b"payload"),
                    AppendOutcome::Written
                ));
            }
        }

        let reset = unsafe { nipworker_subscription_try_reset(handle, sub_id.as_ptr(), 15) };

        assert!(reset, "cursor still at 15, reset should succeed");
        if let Ok(state) = handle_ref.state.lock() {
            if let Ok(mut subscriptions) = state.subscriptions.lock() {
                let subscription = subscriptions.subscriptions.get("sub-1").unwrap();
                assert_eq!(&subscription.bytes()[0..4], &4u32.to_le_bytes());
                // A subsequent append starts writing at offset 4 again.
                assert!(matches!(
                    subscriptions.append_payload("sub-1", b"next"),
                    AppendOutcome::Written
                ));
                let subscription = subscriptions.subscriptions.get("sub-1").unwrap();
                assert_eq!(&subscription.bytes()[4..8], &4u32.to_le_bytes());
                assert_eq!(&subscription.bytes()[8..12], b"next");
                assert_eq!(&subscription.bytes()[0..4], &(12u32).to_le_bytes());
            }
        }
        let _ = unsafe { Box::from_raw(handle as *mut NipworkerHandle) };
    }

    #[test]
    fn try_reset_fails_when_engine_appended_after_expected() {
        let handle = test_handle();
        let sub_id = CString::new("sub-1").unwrap();
        let handle_ref = unsafe { &*(handle as *mut NipworkerHandle) };
        if let Ok(state) = handle_ref.state.lock() {
            if let Ok(mut subscriptions) = state.subscriptions.lock() {
                subscriptions.register("sub-1".to_string(), 64, true);
                assert!(matches!(
                    subscriptions.append_payload("sub-1", b"payload"),
                    AppendOutcome::Written
                ));
                // Engine appends again after the host drained at cursor 15.
                assert!(matches!(
                    subscriptions.append_payload("sub-1", b"more"),
                    AppendOutcome::Written
                ));
            }
        }

        let reset = unsafe { nipworker_subscription_try_reset(handle, sub_id.as_ptr(), 15) };

        assert!(!reset, "cursor moved past 15, reset must fail");
        if let Ok(state) = handle_ref.state.lock() {
            if let Ok(subscriptions) = state.subscriptions.lock() {
                assert_eq!(
                    &subscriptions.subscriptions.get("sub-1").unwrap().bytes()[0..4],
                    &(23u32).to_le_bytes()
                );
            }
        }
        let _ = unsafe { Box::from_raw(handle as *mut NipworkerHandle) };
    }

    #[test]
    fn try_reset_returns_false_for_unknown_sub_and_null_args() {
        let handle = test_handle();
        let sub_id = CString::new("missing").unwrap();

        let reset = unsafe { nipworker_subscription_try_reset(handle, sub_id.as_ptr(), 4) };
        assert!(!reset, "unknown sub must return false");

        let reset_null_handle =
            unsafe { nipworker_subscription_try_reset(std::ptr::null_mut(), sub_id.as_ptr(), 4) };
        assert!(!reset_null_handle, "null handle must return false");

        let reset_null_sub =
            unsafe { nipworker_subscription_try_reset(handle, std::ptr::null(), 4) };
        assert!(!reset_null_sub, "null sub_id must return false");

        let _ = unsafe { Box::from_raw(handle as *mut NipworkerHandle) };
    }

    #[test]
    fn opaque_pin_retains_only_its_buffer_across_engine_generation() {
        let handle = test_handle();
        let sub_a = CString::new("sub-a").unwrap();
        let sub_b = CString::new("sub-b").unwrap();
        assert!(unsafe { nipworker_register_subscription(handle, sub_a.as_ptr(), 64) });
        assert!(unsafe { nipworker_register_subscription(handle, sub_b.as_ptr(), 128) });

        let (old_store, backing_a_weak, backing_b_weak) = {
            let handle_ref = unsafe { &*(handle as *mut NipworkerHandle) };
            let store = handle_ref.state.lock().unwrap().subscriptions.clone();
            let locked = store.lock().unwrap();
            let backing_a = Arc::downgrade(&locked.subscriptions["sub-a"].backing);
            let backing_b = Arc::downgrade(&locked.subscriptions["sub-b"].backing);
            drop(locked);
            (store, backing_a, backing_b)
        };
        let old_store_weak = Arc::downgrade(&old_store);
        let mut data = std::ptr::null_mut();
        let mut len = 0usize;
        let pin =
            unsafe { nipworker_subscription_pin(handle, sub_a.as_ptr(), &mut data, &mut len) };
        assert!(!pin.is_null());
        assert!(!data.is_null());
        assert_eq!(len, 64);
        drop(old_store);

        nipworker_deinit(handle);

        assert!(old_store_weak.upgrade().is_none());
        assert!(backing_b_weak.upgrade().is_none());
        assert!(backing_a_weak.upgrade().is_some());
        assert_eq!(
            unsafe { std::slice::from_raw_parts(data, 4) },
            &4u32.to_le_bytes()
        );
        unsafe { nipworker_subscription_pin_release(pin) };
        assert!(backing_a_weak.upgrade().is_none());
        let _ = unsafe { Box::from_raw(handle as *mut NipworkerHandle) };
    }

    #[test]
    fn cleanup_removes_logical_subscription_while_buffer_pin_stays_valid() {
        let handle = test_handle();
        let sub_id = CString::new("pinned-sub").unwrap();
        assert!(unsafe { nipworker_register_subscription(handle, sub_id.as_ptr(), 64) });
        let backing_weak = {
            let handle_ref = unsafe { &*(handle as *mut NipworkerHandle) };
            let state = handle_ref.state.lock().unwrap();
            let subscriptions = state.subscriptions.lock().unwrap();
            Arc::downgrade(&subscriptions.subscriptions["pinned-sub"].backing)
        };
        let mut data = std::ptr::null_mut();
        let mut len = 0usize;
        let pin =
            unsafe { nipworker_subscription_pin(handle, sub_id.as_ptr(), &mut data, &mut len) };
        assert!(!pin.is_null());

        unsafe { nipworker_release_subscription(handle, sub_id.as_ptr()) };
        unsafe { nipworker_cleanup_subscriptions(handle) };
        let handle_ref = unsafe { &*(handle as *mut NipworkerHandle) };
        assert!(!handle_ref
            .state
            .lock()
            .unwrap()
            .subscriptions
            .lock()
            .unwrap()
            .subscriptions
            .contains_key("pinned-sub"));
        assert!(backing_weak.upgrade().is_some());
        assert_eq!(
            unsafe { std::slice::from_raw_parts(data, 4) },
            &4u32.to_le_bytes()
        );

        unsafe { nipworker_subscription_pin_release(pin) };
        assert!(backing_weak.upgrade().is_none());
        let _ = unsafe { Box::from_raw(handle as *mut NipworkerHandle) };
    }

    #[test]
    fn repeated_init_deinit_stops_callbacks_before_return() {
        const SECRET: &str = "f7e69dd87239da6a828fb9a2fbf481b5b9e147edb848497620e8dc6f5ec10a0a";
        let secret = CString::new(SECRET).unwrap();

        for _ in 0..3 {
            let callbacks = Box::new(AtomicUsize::new(0));
            let userdata = Box::into_raw(callbacks);
            let handle = nipworker_init(counting_callback, userdata.cast());
            assert!(!handle.is_null());
            unsafe { nipworker_set_private_key(handle, secret.as_ptr()) };

            let deadline = Instant::now() + Duration::from_secs(2);
            while unsafe { &*userdata }.load(Ordering::Acquire) == 0 && Instant::now() < deadline {
                std::thread::yield_now();
            }
            assert!(
                unsafe { &*userdata }.load(Ordering::Acquire) > 0,
                "callback path should be active before teardown"
            );

            nipworker_deinit(handle);
            let callbacks_after_deinit = unsafe { &*userdata }.load(Ordering::Acquire);
            // Simulate late calls from a stale native/runtime generation. The
            // tombstone rejects them and the joined callback task cannot fire.
            unsafe { nipworker_set_private_key(handle, secret.as_ptr()) };
            std::thread::sleep(Duration::from_millis(20));
            assert_eq!(
                unsafe { &*userdata }.load(Ordering::Acquire),
                callbacks_after_deinit
            );

            let _ = unsafe { Box::from_raw(userdata) };
            let _ = unsafe { Box::from_raw(handle as *mut NipworkerHandle) };
        }
    }

    #[test]
    fn shared_registry_prevents_overlap_and_anchor_survives_runtime_reload() {
        const SECRET: &str = "f7e69dd87239da6a828fb9a2fbf481b5b9e147edb848497620e8dc6f5ec10a0a";
        let secret = CString::new(SECRET).unwrap();
        let first_callbacks = Box::into_raw(Box::new(AtomicUsize::new(0)));
        let second_callbacks = Box::into_raw(Box::new(AtomicUsize::new(0)));
        let initializations_before = SHARED_ENGINE_INITIALIZATIONS.load(Ordering::Acquire);

        let first = nipworker_shared_acquire(
            counting_callback,
            first_callbacks.cast(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            false,
        );
        let second = nipworker_shared_acquire(
            counting_callback,
            second_callbacks.cast(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            false,
        );
        assert!(!first.is_null());
        assert_eq!(
            first, second,
            "native and RN clients must share one engine handle"
        );
        assert_eq!(
            SHARED_ENGINE_INITIALIZATIONS.load(Ordering::Acquire),
            initializations_before + 1,
            "two process clients must initialize exactly one engine"
        );

        unsafe { nipworker_set_private_key(first, secret.as_ptr()) };
        let deadline = Instant::now() + Duration::from_secs(2);
        while (unsafe { &*first_callbacks }.load(Ordering::Acquire) == 0
            || unsafe { &*second_callbacks }.load(Ordering::Acquire) == 0)
            && Instant::now() < deadline
        {
            std::thread::yield_now();
        }
        assert!(unsafe { &*first_callbacks }.load(Ordering::Acquire) > 0);
        assert!(unsafe { &*second_callbacks }.load(Ordering::Acquire) > 0);

        nipworker_shared_release(first, counting_callback, first_callbacks.cast());
        let first_after_release = unsafe { &*first_callbacks }.load(Ordering::Acquire);
        let second_before_retry = unsafe { &*second_callbacks }.load(Ordering::Acquire);
        unsafe { nipworker_clear_signer(second) };
        unsafe { nipworker_set_private_key(second, secret.as_ptr()) };
        let deadline = Instant::now() + Duration::from_secs(2);
        while unsafe { &*second_callbacks }.load(Ordering::Acquire) == second_before_retry
            && Instant::now() < deadline
        {
            std::thread::yield_now();
        }
        assert_eq!(
            unsafe { &*first_callbacks }.load(Ordering::Acquire),
            first_after_release,
            "released client received a stale callback"
        );
        assert!(unsafe { &*second_callbacks }.load(Ordering::Acquire) > second_before_retry);

        nipworker_shared_release(second, counting_callback, second_callbacks.cast());
        let generation_after_final_release = {
            let (mutex, _) = shared_engine();
            let state = mutex.lock().unwrap();
            assert_eq!(
                state.handle, 0,
                "unanchored final release must clear the active handle"
            );
            assert!(
                state.clients.is_empty(),
                "final release must remove every client"
            );
            state.generation
        };

        let third_callbacks = Box::into_raw(Box::new(AtomicUsize::new(0)));
        let third = nipworker_shared_acquire(
            counting_callback,
            third_callbacks.cast(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            false,
        );
        assert!(!third.is_null());
        {
            let (mutex, _) = shared_engine();
            let state = mutex.lock().unwrap();
            assert_eq!(state.handle, third as usize);
            assert_eq!(state.clients.len(), 1);
            assert_eq!(state.generation, generation_after_final_release + 1);
        }
        assert_eq!(
            SHARED_ENGINE_INITIALIZATIONS.load(Ordering::Acquire),
            initializations_before + 2,
            "reacquire after final release must create one sequential generation"
        );

        let anchored = nipworker_shared_process_acquire(
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            false,
        );
        assert_eq!(anchored, third);
        nipworker_shared_release(third, counting_callback, third_callbacks.cast());
        {
            let (mutex, _) = shared_engine();
            let state = mutex.lock().unwrap();
            assert_eq!(state.handle, anchored as usize);
            assert!(state.process_anchored);
            assert!(state.clients.is_empty());
        }

        let fourth_callbacks = Box::into_raw(Box::new(AtomicUsize::new(0)));
        let fourth = nipworker_shared_acquire(
            counting_callback,
            fourth_callbacks.cast(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            false,
        );
        assert_eq!(
            fourth, anchored,
            "runtime reload must reuse the anchored engine"
        );
        assert_eq!(
            SHARED_ENGINE_INITIALIZATIONS.load(Ordering::Acquire),
            initializations_before + 2,
            "runtime reload must not initialize a third engine"
        );
        nipworker_shared_release(fourth, counting_callback, fourth_callbacks.cast());
        nipworker_shared_process_release();
        {
            let (mutex, _) = shared_engine();
            let state = mutex.lock().unwrap();
            assert_eq!(
                state.handle, 0,
                "explicit process release must stop the engine"
            );
            assert!(!state.process_anchored);
            assert!(state.clients.is_empty());
        }

        let _ = unsafe { Box::from_raw(first_callbacks) };
        let _ = unsafe { Box::from_raw(second_callbacks) };
        let _ = unsafe { Box::from_raw(third_callbacks) };
        let _ = unsafe { Box::from_raw(fourth_callbacks) };
    }
}
