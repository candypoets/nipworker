//! Platform-neutral peer byte transport. Swift/Kotlin report peer lifecycle
//! and characteristic bytes; this module owns MTU-aware framing and queues.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::framing::{Fragmenter, OutboundQueue, Reassembler};
use crate::{MeshError, NostrFrame};

#[derive(Clone, Debug)]
pub struct TransportConfig {
    pub max_message_size: usize,
    pub reassembly_timeout: Duration,
    pub max_inflight_messages: usize,
    pub max_inflight_bytes: usize,
    pub max_outbound_frames: usize,
    pub max_outbound_bytes: usize,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            max_message_size: 64 * 1024,
            reassembly_timeout: Duration::from_secs(30),
            max_inflight_messages: 8,
            max_inflight_bytes: 256 * 1024,
            max_outbound_frames: 1024,
            max_outbound_bytes: 512 * 1024,
        }
    }
}

struct PeerLink {
    mtu: usize,
    reassembler: Reassembler,
    outbound: OutboundQueue,
}

pub struct MeshByteTransport {
    config: TransportConfig,
    fragmenter: Fragmenter,
    peers: HashMap<String, PeerLink>,
}

impl MeshByteTransport {
    pub fn new(config: TransportConfig) -> Self {
        Self {
            fragmenter: Fragmenter::new(config.max_message_size),
            config,
            peers: HashMap::new(),
        }
    }

    pub fn peer_connected(&mut self, peer_id: String, mtu: usize) -> Result<(), MeshError> {
        if mtu <= 17 {
            return Err(MeshError::Frame("peer MTU too small".to_string()));
        }
        self.peers.insert(
            peer_id,
            PeerLink {
                mtu,
                reassembler: Reassembler::new(
                    self.config.reassembly_timeout,
                    self.config.max_message_size,
                    self.config.max_inflight_messages,
                    self.config.max_inflight_bytes,
                ),
                outbound: OutboundQueue::new(
                    self.config.max_outbound_frames,
                    self.config.max_outbound_bytes,
                ),
            },
        );
        Ok(())
    }

    pub fn peer_disconnected(&mut self, peer_id: &str) {
        self.peers.remove(peer_id);
    }

    pub fn send(&mut self, peer_id: &str, frame: &NostrFrame) -> Result<(), MeshError> {
        let peer = self
            .peers
            .get_mut(peer_id)
            .ok_or_else(|| MeshError::Frame(format!("unknown peer {peer_id}")))?;
        let fragments = self
            .fragmenter
            .fragment(frame.as_str().as_bytes(), peer.mtu)?;
        peer.outbound.enqueue(fragments)
    }

    pub fn pop_outbound(&mut self, peer_id: &str) -> Option<Vec<u8>> {
        self.peers.get_mut(peer_id)?.outbound.pop()
    }

    pub fn receive(
        &mut self,
        peer_id: &str,
        bytes: &[u8],
        now: Instant,
    ) -> Result<Option<NostrFrame>, MeshError> {
        let peer = self
            .peers
            .get_mut(peer_id)
            .ok_or_else(|| MeshError::Frame(format!("unknown peer {peer_id}")))?;
        let Some(message) = peer.reassembler.push(bytes, now)? else {
            return Ok(None);
        };
        let frame = String::from_utf8(message)
            .map_err(|_| MeshError::Frame("Nostr frame is not UTF-8".to_string()))?;
        NostrFrame::from_string(frame).map(Some)
    }

    pub fn connected_peer_count(&self) -> usize {
        self.peers.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_platform_links_exchange_a_fragmented_nostr_frame() {
        let mut a = MeshByteTransport::new(TransportConfig::default());
        let mut b = MeshByteTransport::new(TransportConfig::default());
        a.peer_connected("b".to_string(), 100).unwrap();
        b.peer_connected("a".to_string(), 100).unwrap();
        let frame = NostrFrame::from_string(
            serde_json::json!(["EVENT", { "content": "x".repeat(1000) }]).to_string(),
        )
        .unwrap();
        a.send("b", &frame).unwrap();

        let now = Instant::now();
        let mut received = None;
        while let Some(fragment) = a.pop_outbound("b") {
            if let Some(value) = b.receive("a", &fragment, now).unwrap() {
                received = Some(value);
            }
        }
        assert_eq!(received.unwrap(), frame);
    }

    #[test]
    fn disconnect_drops_queues_and_reassembly_state() {
        let mut transport = MeshByteTransport::new(TransportConfig::default());
        transport.peer_connected("peer".to_string(), 100).unwrap();
        let frame = NostrFrame::from_string(serde_json::json!(["EOSE", "s"]).to_string()).unwrap();
        transport.send("peer", &frame).unwrap();
        transport.peer_disconnected("peer");
        assert_eq!(transport.connected_peer_count(), 0);
        assert!(transport.pop_outbound("peer").is_none());
    }
}
