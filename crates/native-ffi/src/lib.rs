#[cfg(target_os = "android")]
mod jni;
mod mesh_ffi;

use futures::StreamExt;
use nipworker_core::service::engine::NostrEngine;
use nipworker_core::storage::{NostrDbStorage, PersistentNostrDbStorage};
use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr};
use std::path::PathBuf;
use std::slice;
use std::sync::{Arc, Mutex};
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
    Wake,
}

struct NativeSubscription {
    buffer: Vec<u8>,
    ref_count: i32,
    close_on_cleanup: bool,
}

impl NativeSubscription {
    fn new(buffer_size: usize, close_on_cleanup: bool) -> Self {
        let size = buffer_size.max(4);
        let mut buffer = vec![0; size];
        buffer[0..4].copy_from_slice(&4u32.to_le_bytes());
        Self {
            buffer,
            ref_count: 1,
            close_on_cleanup,
        }
    }

    fn append_payload(&mut self, payload: &[u8]) -> bool {
        let write_pos = u32::from_le_bytes([
            self.buffer[0],
            self.buffer[1],
            self.buffer[2],
            self.buffer[3],
        ]) as usize;
        let required = 4 + payload.len();
        if write_pos + required > self.buffer.len() {
            return false;
        }
        self.buffer[write_pos..write_pos + 4]
            .copy_from_slice(&(payload.len() as u32).to_le_bytes());
        self.buffer[write_pos + 4..write_pos + 4 + payload.len()].copy_from_slice(payload);
        let next_pos = (write_pos + required) as u32;
        self.buffer[0..4].copy_from_slice(&next_pos.to_le_bytes());
        true
    }
}

struct NativeSubscriptionStore {
    subscriptions: HashMap<String, NativeSubscription>,
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

    fn append_payload(&mut self, sub_id: &str, payload: &[u8]) -> Result<(), bool> {
        if let Some(existing) = self.subscriptions.get_mut(sub_id) {
            let written = existing.append_payload(payload);
            if !written {
                let close_on_cleanup = existing.close_on_cleanup;
                return Err(close_on_cleanup);
            }
        }
        Ok(())
    }
}

struct NipworkerState {
    destroyed: bool,
    cmd_tx: Option<UnboundedSender<EngineCommand>>,
    subscriptions: Arc<Mutex<NativeSubscriptionStore>>,
    mesh_tx: Option<tokio::sync::mpsc::UnboundedSender<mesh_ffi::MeshCommand>>,
}

/// Opaque handle
pub struct NipworkerHandle {
    state: Mutex<NipworkerState>,
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
        use tracing_subscriber::filter::LevelFilter;
        use tracing_subscriber::prelude::*;
        let _ = tracing_log::LogTracer::init();
        let _ = tracing_subscriber::registry()
            .with(
                tracing_oslog::OsLogger::new("com.nutscash.sparkling", "nipworker")
                    .with_filter(LevelFilter::ERROR),
            )
            .try_init();
    }
    #[cfg(target_os = "android")]
    {
        android_logger::init_once(
            android_logger::Config::default().with_max_level(log::LevelFilter::Error),
        );
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

    // Cast userdata to usize so it can be moved into the spawned thread.
    let userdata = userdata as usize;
    let callback_subscriptions = subscriptions.clone();
    let callback_cmd_tx = cmd_tx.clone();

    // Spawn engine thread
    thread::spawn(move || {
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

            // Bridge async events to sync callback thread.
            // The callback receives an owned buffer; the host must call
            // nipworker_free_bytes() after copying to avoid leaking memory.
            tokio::task::spawn_local(async move {
                while let Some((sub_id, bytes)) = async_event_rx.next().await {
                    let mut should_forward = true;
                    let mut should_unsubscribe = false;
                    let sub_id_len = sub_id.len();
                    let payload_len = bytes.len();
                    // Direct crypto responses are delivered as callback payloads and do not
                    // own a registered subscription buffer.
                    if !sub_id.is_empty() && sub_id != "crypto" {
                        if let Ok(mut subscriptions) = callback_subscriptions.lock() {
                            match subscriptions.append_payload(&sub_id, &bytes) {
                                Ok(()) => {}
                                Err(close_on_cleanup) => {
                                    should_forward = false;
                                    should_unsubscribe = close_on_cleanup;
                                }
                            }
                        }
                    }
                    if !should_forward {
                        log::warn!(
                            "[nipworker-native] native buffer full for subId={} (subIdLen={}, payloadLen={})",
                            sub_id,
                            sub_id_len,
                            payload_len
                        );
                        if should_unsubscribe {
                            let _ = callback_cmd_tx.send(EngineCommand::HandleMessage(
                                build_unsubscribe_message(&sub_id),
                            ));
                        }
                        continue;
                    }
                    log::debug!(
                        "[nipworker-native] queueing callback for subId={} (subIdLen={}, payloadLen={})",
                        sub_id,
                        sub_id_len,
                        payload_len
                    );
                    let callback_bytes = if sub_id.is_empty() || sub_id == "crypto" {
                        bytes
                    } else {
                        build_route_wake_frame(&sub_id)
                    };
                    let len = callback_bytes.len();
                    let ptr = Box::into_raw(callback_bytes.into_boxed_slice()) as *const u8;
                    callback(userdata as *mut c_void, ptr, len);
                }
            });

            // Process commands asynchronously so the LocalSet isn't blocked
            while let Some(cmd) = cmd_rx.recv().await {
                match cmd {
                    EngineCommand::HandleMessage(bytes) => {
                        let engine = engine.clone();
                        tokio::task::spawn_local(async move {
                            if let Err(e) = engine.handle_message(&bytes).await {
                                log::warn!("[nipworker-native] handle_message error: {}", e);
                            }
                        });
                    }
                    EngineCommand::Wake => {
                        engine.wake();
                    }
                }
            }
        });

        rt.block_on(local);
    });

    let handle = Box::new(NipworkerHandle {
        state: Mutex::new(NipworkerState {
            destroyed: false,
            cmd_tx: Some(cmd_tx),
            subscriptions,
            mesh_tx,
        }),
    });
    Box::into_raw(handle) as *mut c_void
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
                return subscription.buffer.as_mut_ptr();
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
                return subscription.buffer.len();
            }
        }
    }
    0
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

/// Free a buffer previously passed to the callback in `nipworker_init`.
/// The host must call this after copying the data to its own storage.
#[no_mangle]
pub unsafe extern "C" fn nipworker_free_bytes(ptr: *mut u8, len: usize) {
    if !ptr.is_null() && len > 0 {
        let _ = unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)) };
    }
}

#[no_mangle]
pub extern "C" fn nipworker_deinit(handle: *mut c_void) {
    if !handle.is_null() {
        let handle = unsafe { &*(handle as *mut NipworkerHandle) };
        // Mark destroyed and drop the sender to close the channel and unblock the engine thread.
        if let Ok(mut state) = handle.state.lock() {
            state.destroyed = true;
            let _ = state.cmd_tx.take();
        }
        // Intentionally leak the Box to prevent use-after-free from other threads.
    }
}

#[cfg(test)]
mod tests {
    use super::{NativeSubscription, NativeSubscriptionStore};

    #[test]
    fn new_subscription_initializes_header_to_four() {
        let subscription = NativeSubscription::new(64, false);
        assert_eq!(subscription.buffer.len(), 64);
        assert_eq!(&subscription.buffer[0..4], &4u32.to_le_bytes());
    }

    #[test]
    fn append_payload_records_length_and_advances_cursor() {
        let mut subscription = NativeSubscription::new(64, false);
        let payload = b"\x0a\x0b\x0c\x0d";

        let written = subscription.append_payload(payload);
        assert!(written, "payload should fit");
        assert_eq!(
            &subscription.buffer[4..8],
            &(payload.len() as u32).to_le_bytes()
        );
        assert_eq!(&subscription.buffer[8..12], payload);
        assert_eq!(&subscription.buffer[0..4], &(12u32).to_le_bytes());
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
        assert_eq!(&subscription.buffer[0..4], &4u32.to_le_bytes());
    }

    #[test]
    fn register_reuses_existing_subscription_without_reset() {
        let mut store = NativeSubscriptionStore::new();
        store.register("sub-1".to_string(), 32, false);
        assert_eq!(store.subscriptions.get("sub-1").unwrap().ref_count, 1);

        let first = store.subscriptions.get("sub-1").unwrap().buffer[0];
        store.register("sub-1".to_string(), 32, false);
        let second = store.subscriptions.get("sub-1").unwrap();

        assert_eq!(second.ref_count, 2);
        assert_eq!(second.buffer[0], first);
    }

    #[test]
    fn overflow_keeps_subscription_buffer_for_reader_consistency() {
        let mut store = NativeSubscriptionStore::new();
        store.register("sub-1".to_string(), 12, true);
        let large = vec![0u8; 16];

        let result = store.append_payload("sub-1", &large);

        assert_eq!(result, Err(true));
        assert!(store.subscriptions.contains_key("sub-1"));
        assert_eq!(
            &store.subscriptions.get("sub-1").unwrap().buffer[0..4],
            &4u32.to_le_bytes()
        );
    }
}
