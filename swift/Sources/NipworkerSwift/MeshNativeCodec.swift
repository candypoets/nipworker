import Foundation

/// Thin owner of the Rust BLE framing handle. CoreBluetooth remains entirely
/// on the Swift side; this object only reports peer lifecycle and moves bytes.
final class MeshNativeCodec {
    private var handle: UnsafeMutableRawPointer?

    init?(engineHandle: UnsafeMutableRawPointer?) {
        guard let handle = nipworker_mesh_init(engineHandle) else { return nil }
        self.handle = handle
    }

    deinit {
        nipworker_mesh_deinit(handle)
        handle = nil
    }

    func peerConnected(_ peerID: String, mtu: Int) -> Bool {
        peerID.withCString { nipworker_mesh_peer_connected(handle, $0, mtu) }
    }

    func peerDisconnected(_ peerID: String) {
        peerID.withCString { nipworker_mesh_peer_disconnected(handle, $0) }
    }

    func popOutbound(for peerID: String) -> Data? {
        peerID.withCString { peer in
            var length = 0
            guard let pointer = nipworker_mesh_pop_outbound(handle, peer, &length) else {
                return nil
            }
            let data = Data(bytes: pointer, count: length)
            nipworker_free_bytes(pointer, length)
            return data
        }
    }

    func receive(fragment: Data, from peerID: String) -> Bool {
        peerID.withCString { peer in
            fragment.withUnsafeBytes { bytes in
                nipworker_mesh_receive_fragment(
                    handle,
                    peer,
                    bytes.bindMemory(to: UInt8.self).baseAddress,
                    bytes.count
                )
            }
        }
    }
}
