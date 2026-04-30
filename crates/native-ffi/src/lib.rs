#[cfg(target_os = "android")]
mod jni;

use std::ffi::{c_char, c_void, CStr};
use std::slice;
use std::sync::{Arc, Mutex};
use std::thread;
use tokio::runtime::Builder;
use tokio::task::LocalSet;
use nipworker_core::service::engine::NostrEngine;
use futures::StreamExt;
use tokio::sync::mpsc::UnboundedSender;

pub mod transport;
pub mod storage;

use transport::NativeTransport;
use storage::InMemoryStorage;

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
    use nipworker_core::generated::nostr::fb;
    use flatbuffers::FlatBufferBuilder;

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
    // Initialize tracing subscriber for native builds
    #[cfg(target_vendor = "apple")]
    {
        use tracing_subscriber::prelude::*;
        let _ = tracing_subscriber::registry()
            .with(tracing_oslog::OsLogger::new("com.nutscash.sparkling", "nipworker"))
            .try_init();
    }
    #[cfg(not(target_vendor = "apple"))]
    {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .try_init();
    }

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
        let rt = Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let local = LocalSet::new();

        local.spawn_local(async move {
            let transport = Arc::new(NativeTransport::new());
            let storage = Arc::new(InMemoryStorage::new());

            let (async_event_tx, mut async_event_rx) =
                futures::channel::mpsc::channel::<(String, Vec<u8>)>(256);

            let engine = Arc::new(NostrEngine::new(
                transport,
                storage,
                async_event_tx,
            ));

            // Bridge async events to sync callback thread.
            // The callback receives an owned buffer; the host must call
            // nipworker_free_bytes() after copying to avoid leaking memory.
            let callback_ping = callback;
            let userdata_ping = userdata;
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

            // Diagnostic: periodic ping to verify callback path
            tokio::task::spawn_local(async move {
                let mut counter = 0u32;
                loop {
                    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                    counter += 1;
                    let test_msg = format!("ping_{}", counter);
                    let sub_id = "diag";
                    let sub_bytes = sub_id.as_bytes();
                    let msg_bytes = test_msg.as_bytes();
                    let mut payload = Vec::with_capacity(8 + sub_bytes.len() + msg_bytes.len());
                    payload.extend_from_slice(&(sub_bytes.len() as u32).to_le_bytes());
                    payload.extend_from_slice(sub_bytes);
                    payload.extend_from_slice(&(msg_bytes.len() as u32).to_le_bytes());
                    payload.extend_from_slice(msg_bytes);
                    let len = payload.len();
                    let ptr = Box::into_raw(payload.into_boxed_slice()) as *const u8;
                    callback_ping(userdata_ping as *mut c_void, ptr, len);
                }
            });

            // Temporary network diagnostic (Apple platforms only)
            #[cfg(target_vendor = "apple")]
            {
                tokio::task::spawn_local(async {
                    use std::io::Write;
                    let mut log = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open("/tmp/nipworker_diag.log")
                        .unwrap();
                    let _ = writeln!(log, "=== DIAG START ===");

                    // Give the engine a moment to start up
                    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                    let _ = writeln!(log, "Slept 2s");

                    // Test 1: DNS resolution
                    match tokio::net::lookup_host("relay.damus.io:443").await {
                        Ok(addrs) => {
                            let addrs: Vec<_> = addrs.collect();
                            let _ = writeln!(log, "DNS OK: {:?}", addrs);
                            tracing::info!("DNS diagnostic: relay.damus.io:443 = {:?}", addrs);
                        }
                        Err(e) => {
                            let _ = writeln!(log, "DNS FAIL: {}", e);
                            tracing::error!("DNS diagnostic failed: {}", e);
                        }
                    }

                    // Test 2: Raw TCP to known IPv4 address
                    match tokio::net::TcpStream::connect("104.21.61.55:443").await {
                        Ok(_) => {
                            let _ = writeln!(log, "TCP IPv4 OK");
                            tracing::info!("TCP diagnostic: IPv4 connection OK");
                        }
                        Err(e) => {
                            let _ = writeln!(log, "TCP IPv4 FAIL: {}", e);
                            tracing::error!("TCP diagnostic IPv4 failed: {}", e);
                        }
                    }

                    // Test 3: Raw TCP to hostname
                    match tokio::net::TcpStream::connect("relay.damus.io:443").await {
                        Ok(_) => {
                            let _ = writeln!(log, "TCP HOST OK");
                            tracing::info!("TCP diagnostic: hostname connection OK");
                        }
                        Err(e) => {
                            let _ = writeln!(log, "TCP HOST FAIL: {}", e);
                            tracing::error!("TCP diagnostic hostname failed: {}", e);
                        }
                    }

                    // Test 4: Direct WebSocket connect
                    let _ = writeln!(log, "WS about to connect");
                    tracing::info!("WS diagnostic: about to call connect_async to wss://relay.damus.io");
                    match tokio::time::timeout(
                        tokio::time::Duration::from_secs(10),
                        tokio_tungstenite::connect_async("wss://relay.damus.io"),
                    ).await
                    {
                        Ok(Ok(_)) => {
                            let _ = writeln!(log, "WS OK");
                            tracing::info!("WS diagnostic: connect_async SUCCESS");
                        }
                        Ok(Err(e)) => {
                            let _ = writeln!(log, "WS ERROR: {}", e);
                            tracing::error!("WS diagnostic: connect_async ERROR: {}", e);
                        }
                        Err(_) => {
                            let _ = writeln!(log, "WS TIMEOUT");
                            tracing::error!("WS diagnostic: connect_async TIMEOUT");
                        }
                    }
                    let _ = writeln!(log, "=== DIAG END ===");
                });
            }

            // Process commands asynchronously so the LocalSet isn't blocked
            while let Some(cmd) = cmd_rx.recv().await {
                match cmd {
                    EngineCommand::HandleMessage(bytes) => {
                        let engine = engine.clone();
                        tokio::task::spawn_local(async move {
                            if let Err(e) = engine.handle_message(&bytes).await {
                                tracing::warn!("[nipworker-native] handle_message error: {}", e);
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
