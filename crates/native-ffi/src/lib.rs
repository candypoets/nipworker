#[cfg(target_os = "android")]
mod jni;

use futures::StreamExt;
use nipworker_core::service::engine::NostrEngine;
use nipworker_core::storage::{NostrDbStorage, PersistentNostrDbStorage};
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

fn new_core_storage(max_buffer_size: usize) -> NostrDbStorage {
    NostrDbStorage::new(
        "nipworker".to_string(),
        max_buffer_size,
        DEFAULT_RELAYS.iter().map(|s| s.to_string()).collect(),
        INDEXER_RELAYS.iter().map(|s| s.to_string()).collect(),
    )
}

/// Commands sent to the engine thread
enum EngineCommand {
    HandleMessage(Vec<u8>),
}

struct NipworkerState {
    destroyed: bool,
    cmd_tx: Option<UnboundedSender<EngineCommand>>,
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
    // Initialize tracing subscriber for native builds
    #[cfg(target_vendor = "apple")]
    {
        use tracing_subscriber::prelude::*;
        let _ = tracing_log::LogTracer::init();
        let _ = tracing_subscriber::registry()
            .with(tracing_oslog::OsLogger::new(
                "com.nutscash.sparkling",
                "nipworker",
            ))
            .try_init();
    }
    #[cfg(target_os = "android")]
    {
        android_logger::init_once(
            android_logger::Config::default().with_max_level(log::LevelFilter::Debug),
        );
        log::info!("android_logger initialized");
    }
    #[cfg(all(not(target_vendor = "apple"), not(target_os = "android")))]
    {
        let _ = tracing_log::LogTracer::init();
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
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

    // Set panic hook so Rust panics are visible instead of silent thread death
    std::panic::set_hook(Box::new(|info| {
        let backtrace = std::backtrace::Backtrace::capture();
        eprintln!("[nipworker] PANIC: {}", info);
        eprintln!("[nipworker] Backtrace:\n{}", backtrace);
    }));

    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<EngineCommand>();

    // Cast userdata to usize so it can be moved into the spawned thread.
    let userdata = userdata as usize;

    // Spawn engine thread
    thread::spawn(move || {
        let rt = Builder::new_current_thread().enable_all().build().unwrap();

        let local = LocalSet::new();

        local.spawn_local(async move {
            let (async_event_tx, mut async_event_rx) =
                futures::channel::mpsc::channel::<(String, Vec<u8>)>(256);

            let storage_path = storage_path.clone();
            let engine = Arc::new(NostrEngine::new_threaded(
                || Arc::new(NativeTransport::new()),
                move || {
                    if let Some(path) = storage_path.clone() {
                        Arc::new(PersistentNostrDbStorage::new(
                            new_core_storage(8 * 1024 * 1024),
                            FileBlobStore::new(path),
                        )) as Arc<dyn nipworker_core::traits::Storage>
                    } else {
                        Arc::new(new_core_storage(8 * 1024 * 1024))
                            as Arc<dyn nipworker_core::traits::Storage>
                    }
                },
                async_event_tx,
            ));

            // Bridge async events to sync callback thread.
            // The callback receives an owned buffer; the host must call
            // nipworker_free_bytes() after copying to avoid leaking memory.
            tokio::task::spawn_local(async move {
                while let Some((sub_id, bytes)) = async_event_rx.next().await {
                    let sub_id_bytes = sub_id.as_bytes();
                    let mut payload = Vec::with_capacity(8 + sub_id_bytes.len() + bytes.len());
                    payload.extend_from_slice(&(sub_id_bytes.len() as u32).to_le_bytes());
                    payload.extend_from_slice(sub_id_bytes);
                    payload.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
                    payload.extend_from_slice(&bytes);
                    let len = payload.len();
                    let ptr = Box::into_raw(payload.into_boxed_slice()) as *const u8;
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
                }
            }
        });

        rt.block_on(local);
    });

    let handle = Box::new(NipworkerHandle {
        state: Mutex::new(NipworkerState {
            destroyed: false,
            cmd_tx: Some(cmd_tx),
        }),
    });
    Box::into_raw(handle) as *mut c_void
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
