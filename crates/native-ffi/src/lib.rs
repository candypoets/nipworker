use std::ffi::{c_char, c_void, CStr};
use std::slice;
use std::sync::Arc;
use std::thread;
use tokio::runtime::Builder;
use tokio::task::LocalSet;
use nipworker_core::service::engine::NostrEngine;
use futures::StreamExt;

pub mod transport;
pub mod storage;
pub mod signer;

use transport::NativeTransport;
use storage::InMemoryStorage;
use signer::NativeSigner;

/// Commands sent to the engine thread
enum EngineCommand {
    HandleMessage(Vec<u8>),
    SetPrivateKey(String),
}

/// Opaque handle
pub struct NipworkerHandle {
    cmd_tx: std::sync::mpsc::Sender<EngineCommand>,
}

#[no_mangle]
pub extern "C" fn nipworker_init(
    callback: extern "C" fn(*const u8, usize),
) -> *mut c_void {
    let (_event_tx, _event_rx) = std::sync::mpsc::channel::<(String, Vec<u8>)>();
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<EngineCommand>();

    // Spawn engine thread
    thread::spawn(move || {
        let rt = Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let local = LocalSet::new();

        local.spawn_local(async move {
            let transport = Arc::new(NativeTransport::new());
            let storage = Arc::new(InMemoryStorage::new());
            let signer = Arc::new(NativeSigner::new());

            let (async_event_tx, mut async_event_rx) =
                futures::channel::mpsc::channel::<(String, Vec<u8>)>(256);

            let engine = Arc::new(NostrEngine::new(
                transport,
                storage,
                signer.clone(),
                async_event_tx,
            ));

            // Bridge async events to sync callback thread
            tokio::task::spawn_local(async move {
                while let Some((sub_id, bytes)) = async_event_rx.next().await {
                    let sub_id_bytes = sub_id.as_bytes();
                    let mut payload = Vec::with_capacity(8 + sub_id_bytes.len() + bytes.len());
                    payload.extend_from_slice(&(sub_id_bytes.len() as u32).to_le_bytes());
                    payload.extend_from_slice(sub_id_bytes);
                    payload.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
                    payload.extend_from_slice(&bytes);
                    callback(payload.as_ptr(), payload.len());
                }
            });

            // Process commands
            while let Ok(cmd) = cmd_rx.recv() {
                match cmd {
                    EngineCommand::HandleMessage(bytes) => {
                        let engine = engine.clone();
                        tokio::task::spawn_local(async move {
                            if let Err(e) = engine.handle_message(&bytes).await {
                                tracing::warn!("[nipworker-native] handle_message error: {}", e);
                            }
                        });
                    }
                    EngineCommand::SetPrivateKey(secret) => {
                        if let Err(e) = signer.set_private_key(&secret) {
                            tracing::warn!("[nipworker-native] set_private_key error: {}", e);
                        }
                    }
                }
            }
        });

        rt.block_on(local);
    });

    let handle = Box::new(NipworkerHandle { cmd_tx });
    Box::into_raw(handle) as *mut c_void
}

#[no_mangle]
pub extern "C" fn nipworker_handle_message(handle: *mut c_void, ptr: *const u8, len: usize) {
    if handle.is_null() || ptr.is_null() {
        return;
    }
    let handle = unsafe { &*(handle as *mut NipworkerHandle) };
    let bytes = unsafe { slice::from_raw_parts(ptr, len) }.to_vec();
    let _ = handle.cmd_tx.send(EngineCommand::HandleMessage(bytes));
}

#[no_mangle]
pub extern "C" fn nipworker_set_private_key(handle: *mut c_void, ptr: *const c_char) {
    if handle.is_null() || ptr.is_null() {
        return;
    }
    let handle = unsafe { &*(handle as *mut NipworkerHandle) };
    let secret = unsafe { CStr::from_ptr(ptr) }.to_string_lossy().to_string();
    let _ = handle.cmd_tx.send(EngineCommand::SetPrivateKey(secret));
}

#[no_mangle]
pub extern "C" fn nipworker_deinit(handle: *mut c_void) {
    if !handle.is_null() {
        let _ = unsafe { Box::from_raw(handle as *mut NipworkerHandle) };
    }
}
