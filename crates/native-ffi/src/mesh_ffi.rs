//! C ABI for the platform BLE byte boundary.

use std::ffi::{c_char, c_void, CStr};
use std::sync::mpsc;

use nipworker_mesh::cache::MeshCacheClient;
use nipworker_mesh::runtime::MeshRuntime;
use nipworker_mesh::CanonicalEvent;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::NipworkerHandle;

pub(crate) enum MeshCommand {
    PeerConnected {
        peer: String,
        mtu: usize,
        reply: mpsc::Sender<bool>,
    },
    PeerDisconnected {
        peer: String,
    },
    Receive {
        peer: String,
        fragment: Vec<u8>,
        reply: mpsc::Sender<bool>,
    },
    PopOutbound {
        peer: String,
        reply: mpsc::Sender<Option<Vec<u8>>>,
    },
    SetProfile {
        event: CanonicalEvent,
        reply: mpsc::Sender<bool>,
    },
    ClearProfile {
        reply: mpsc::Sender<bool>,
    },
}

pub(crate) async fn run_mesh_runtime(
    endpoint: nipworker_core::service::engine::MeshCacheEndpoint,
    mut commands: UnboundedReceiver<MeshCommand>,
) {
    let cache = MeshCacheClient::from_native_channels(endpoint.requests, endpoint.responses);
    let mut runtime = MeshRuntime::new(cache);
    while let Some(command) = commands.recv().await {
        match command {
            MeshCommand::PeerConnected { peer, mtu, reply } => {
                let _ = reply.send(runtime.peer_connected(peer, mtu).await.is_ok());
            }
            MeshCommand::PeerDisconnected { peer } => runtime.peer_disconnected(&peer),
            MeshCommand::Receive {
                peer,
                fragment,
                reply,
            } => {
                let _ = reply.send(runtime.receive_fragment(&peer, &fragment).await.is_ok());
            }
            MeshCommand::PopOutbound { peer, reply } => {
                let _ = reply.send(runtime.pop_outbound(&peer));
            }
            MeshCommand::SetProfile { event, reply } => {
                let _ = reply.send(runtime.set_local_profile(event).await.is_ok());
            }
            MeshCommand::ClearProfile { reply } => {
                let _ = reply.send(runtime.clear_local_profile().await.is_ok());
            }
        }
    }
}

fn peer_id(value: *const c_char) -> Option<String> {
    if value.is_null() {
        return None;
    }
    let value = unsafe { CStr::from_ptr(value) }.to_str().ok()?.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn bytes<'a>(value: *const u8, len: usize) -> Option<&'a [u8]> {
    if value.is_null() || len == 0 {
        return None;
    }
    Some(unsafe { std::slice::from_raw_parts(value, len) })
}

fn mesh_sender(handle: *mut c_void) -> Option<UnboundedSender<MeshCommand>> {
    if handle.is_null() {
        return None;
    }
    let handle = unsafe { &*(handle as *mut NipworkerHandle) };
    let state = handle.state.lock().ok()?;
    if state.destroyed {
        None
    } else {
        state.mesh_tx.clone()
    }
}

fn request<T>(
    sender: &UnboundedSender<MeshCommand>,
    build: impl FnOnce(mpsc::Sender<T>) -> MeshCommand,
) -> Option<T> {
    let (reply_tx, reply_rx) = mpsc::channel();
    sender.send(build(reply_tx)).ok()?;
    reply_rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .ok()
}

/// Mesh uses the existing native engine handle. No second cache worker or
/// independent protocol instance is created.
#[no_mangle]
pub extern "C" fn nipworker_mesh_init(handle: *mut c_void) -> *mut c_void {
    mesh_sender(handle)
        .map(|_| handle)
        .unwrap_or(std::ptr::null_mut())
}

#[no_mangle]
pub extern "C" fn nipworker_mesh_peer_connected(
    handle: *mut c_void,
    peer: *const c_char,
    mtu: usize,
) -> bool {
    let (Some(sender), Some(peer)) = (mesh_sender(handle), peer_id(peer)) else {
        return false;
    };
    request(&sender, |reply| MeshCommand::PeerConnected {
        peer,
        mtu,
        reply,
    })
    .unwrap_or(false)
}

#[no_mangle]
pub extern "C" fn nipworker_mesh_peer_disconnected(handle: *mut c_void, peer: *const c_char) {
    let (Some(sender), Some(peer)) = (mesh_sender(handle), peer_id(peer)) else {
        return;
    };
    let _ = sender.send(MeshCommand::PeerDisconnected { peer });
}

#[no_mangle]
pub extern "C" fn nipworker_mesh_set_profile_json(
    handle: *mut c_void,
    profile_json: *const c_char,
) -> bool {
    let Some(sender) = mesh_sender(handle) else {
        return false;
    };
    if profile_json.is_null() {
        return false;
    }
    let Ok(profile_json) = unsafe { CStr::from_ptr(profile_json) }.to_str() else {
        return false;
    };
    let Ok(event) = serde_json::from_str::<CanonicalEvent>(profile_json) else {
        return false;
    };
    request(&sender, |reply| MeshCommand::SetProfile { event, reply }).unwrap_or(false)
}

#[no_mangle]
pub extern "C" fn nipworker_mesh_clear_profile(handle: *mut c_void) -> bool {
    let Some(sender) = mesh_sender(handle) else {
        return false;
    };
    request(&sender, |reply| MeshCommand::ClearProfile { reply }).unwrap_or(false)
}

#[no_mangle]
pub extern "C" fn nipworker_mesh_pop_outbound(
    handle: *mut c_void,
    peer: *const c_char,
    out_len: *mut usize,
) -> *mut u8 {
    let (Some(sender), Some(peer)) = (mesh_sender(handle), peer_id(peer)) else {
        return std::ptr::null_mut();
    };
    let Some(Some(mut value)) = request(&sender, |reply| MeshCommand::PopOutbound { peer, reply })
    else {
        return std::ptr::null_mut();
    };
    if out_len.is_null() || value.is_empty() {
        return std::ptr::null_mut();
    }
    let ptr = value.as_mut_ptr();
    unsafe { *out_len = value.len() };
    std::mem::forget(value);
    ptr
}

#[no_mangle]
pub extern "C" fn nipworker_mesh_receive_fragment(
    handle: *mut c_void,
    peer: *const c_char,
    fragment: *const u8,
    fragment_len: usize,
) -> bool {
    let (Some(sender), Some(peer), Some(fragment)) = (
        mesh_sender(handle),
        peer_id(peer),
        bytes(fragment, fragment_len),
    ) else {
        return false;
    };
    request(&sender, |reply| MeshCommand::Receive {
        peer,
        fragment: fragment.to_vec(),
        reply,
    })
    .unwrap_or(false)
}

/// The mesh handle is borrowed from `NipworkerHandle`; its lifetime is ended
/// by `nipworker_deinit`.
#[no_mangle]
pub extern "C" fn nipworker_mesh_deinit(_handle: *mut c_void) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    extern "C" fn discard_callback(_: *mut c_void, ptr: *const u8, len: usize) {
        if !ptr.is_null() {
            unsafe { crate::nipworker_free_bytes(ptr as *mut u8, len) };
        }
    }

    fn mesh_engine() -> *mut c_void {
        crate::nipworker_init_with_options(
            discard_callback,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            true,
        )
    }

    #[test]
    fn engine_owned_mesh_queues_initial_negentropy_frame() {
        let engine = mesh_engine();
        let mesh = nipworker_mesh_init(engine);
        assert_eq!(mesh, engine);
        let peer = CString::new("peer-a").unwrap();
        assert!(nipworker_mesh_peer_connected(mesh, peer.as_ptr(), 100));

        let mut fragment_count = 0;
        loop {
            let mut len = 0;
            let ptr = nipworker_mesh_pop_outbound(mesh, peer.as_ptr(), &mut len);
            if ptr.is_null() {
                break;
            }
            fragment_count += 1;
            unsafe { crate::nipworker_free_bytes(ptr, len) };
        }
        assert!(fragment_count > 0, "peer connection must queue NEG-OPEN");
        nipworker_mesh_peer_disconnected(mesh, peer.as_ptr());
        crate::nipworker_deinit(engine);
    }

    #[test]
    fn mesh_is_not_allocated_by_default() {
        let engine = crate::nipworker_init(discard_callback, std::ptr::null_mut());
        assert!(nipworker_mesh_init(engine).is_null());
        crate::nipworker_deinit(engine);
    }

    #[test]
    fn two_engine_owned_meshes_complete_nip77_through_cache_workers() {
        let a = mesh_engine();
        let b = mesh_engine();
        let peer_a = CString::new("a").unwrap();
        let peer_b = CString::new("b").unwrap();
        assert!(nipworker_mesh_peer_connected(a, peer_b.as_ptr(), 80));
        assert!(nipworker_mesh_peer_connected(b, peer_a.as_ptr(), 80));

        let mut delivered = 0;
        for _ in 0..10_000 {
            let mut progressed = false;
            progressed |= transfer_one(a, peer_b.as_ptr(), b, peer_a.as_ptr());
            progressed |= transfer_one(b, peer_a.as_ptr(), a, peer_b.as_ptr());
            if progressed {
                delivered += 1;
            } else {
                break;
            }
        }
        assert!(delivered > 0);
        assert!(!transfer_one(a, peer_b.as_ptr(), b, peer_a.as_ptr()));
        assert!(!transfer_one(b, peer_a.as_ptr(), a, peer_b.as_ptr()));
        crate::nipworker_deinit(a);
        crate::nipworker_deinit(b);
    }

    fn transfer_one(
        from: *mut c_void,
        from_peer: *const c_char,
        to: *mut c_void,
        to_peer: *const c_char,
    ) -> bool {
        let mut len = 0;
        let ptr = nipworker_mesh_pop_outbound(from, from_peer, &mut len);
        if ptr.is_null() {
            return false;
        }
        assert!(nipworker_mesh_receive_fragment(to, to_peer, ptr, len));
        unsafe { crate::nipworker_free_bytes(ptr, len) };
        true
    }
}
