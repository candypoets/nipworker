use std::sync::Arc;
use std::time::Duration;
use async_trait::async_trait;
use tokio::task::LocalSet;
use tokio::time::timeout;

use crate::channel::{TokioWorkerChannel, WorkerChannel, ChannelError};
use crate::generated::nostr::fb;
use crate::service::engine::NostrEngine;
use crate::traits::{
    RelayTransport, Signer, Storage, TransportError, TransportStatus,
    StorageError, SignerError,
};
use crate::types::network::Request;
use crate::types::nostr::Template;

// ============================================================================
// Mock Implementations for Timeout Testing
// ============================================================================

/// MockStorage that hangs indefinitely on query()
struct HangingStorage;

#[async_trait(?Send)]
impl Storage for HangingStorage {
    async fn query(&self, _filters: Vec<crate::types::nostr::Filter>) -> Result<Vec<Vec<u8>>, StorageError> {
        // Hang forever using std::future::pending()
        std::future::pending::<()>().await;
        unreachable!()
    }

    async fn persist(&self, _event_bytes: &[u8]) -> Result<(), StorageError> {
        Ok(())
    }

    async fn initialize(&self) -> Result<(), StorageError> {
        Ok(())
    }
}

/// MockRelayTransport where connect() hangs forever
struct HangingRelayTransport;

#[async_trait(?Send)]
impl RelayTransport for HangingRelayTransport {
    async fn connect(&self, _url: &str) -> Result<(), TransportError> {
        // Hang forever
        std::future::pending::<()>().await;
        unreachable!()
    }

    fn disconnect(&self, _url: &str) {
        // No-op
    }

    async fn send(&self, _url: &str, _frame: String) -> Result<(), TransportError> {
        Ok(())
    }

    fn on_message(&self, _url: &str, _callback: Box<dyn Fn(String)>) {
        // No-op
    }

    fn on_status(&self, _url: &str, _callback: Box<dyn Fn(TransportStatus)>) {
        // No-op
    }
}

/// MockSigner where sign_event() takes a long time
struct SlowSigner {
    delay_ms: u64,
}

impl SlowSigner {
    fn with_delay(delay_ms: u64) -> Self {
        Self { delay_ms }
    }
}

#[async_trait(?Send)]
impl Signer for SlowSigner {
    async fn get_public_key(&self) -> Result<String, SignerError> {
        Ok("0000000000000000000000000000000000000000000000000000000000000001".to_string())
    }

    async fn sign_event(&self, _event_json: &str) -> Result<String, SignerError> {
        tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
        Ok("0000000000000000000000000000000000000000000000000000000000000002".to_string())
    }

    async fn nip04_encrypt(&self, _peer: &str, _plaintext: &str) -> Result<String, SignerError> {
        Ok(String::new())
    }

    async fn nip04_decrypt(&self, _peer: &str, _ciphertext: &str) -> Result<String, SignerError> {
        Ok(String::new())
    }

    async fn nip44_encrypt(&self, _peer: &str, _plaintext: &str) -> Result<String, SignerError> {
        Ok(String::new())
    }

    async fn nip44_decrypt(&self, _peer: &str, _ciphertext: &str) -> Result<String, SignerError> {
        Ok(String::new())
    }

    async fn nip04_decrypt_between(
        &self,
        _sender: &str,
        _recipient: &str,
        _ciphertext: &str,
    ) -> Result<String, SignerError> {
        Ok(String::new())
    }

    async fn nip44_decrypt_between(
        &self,
        _sender: &str,
        _recipient: &str,
        _ciphertext: &str,
    ) -> Result<String, SignerError> {
        Ok(String::new())
    }
}

// ============================================================================
// Test 1: Storage Query Timeout
// ============================================================================

#[tokio::test]
async fn test_storage_query_timeout() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let transport = Arc::new(HangingRelayTransport);
            let storage = Arc::new(HangingStorage);
            let signer = Arc::new(SlowSigner::with_delay(1));

            let (event_sink_tx, _event_sink_rx) =
                futures::channel::mpsc::channel::<(String, Vec<u8>)>(100);

            // Create engine - should not block during construction
            let engine = NostrEngine::new(
                transport.clone(),
                storage.clone(),
                signer.clone(),
                event_sink_tx,
            );

            // Send a subscribe request that will trigger cache query
            // The parser should not deadlock even though storage.query() hangs
            let sub_id = "test_sub";
            let request = Request {
                relays: vec!["wss://r".to_string()],
                ..Default::default()
            };

            // Use timeout to ensure this doesn't hang forever
            let result: Result<Result<(), crate::nostr_error::NostrError>, _> = timeout(
                Duration::from_millis(100),
                engine.subscribe(sub_id.to_string(), vec![request]),
            )
            .await;

            // The subscribe call should complete quickly (it just sends a message)
            // The actual cache query happens asynchronously in the background
            assert!(result.is_ok(), "subscribe should complete within timeout");
            assert!(result.unwrap().is_ok(), "subscribe should succeed");

            // Give the system a moment to process, then verify no deadlock
            tokio::time::sleep(Duration::from_millis(50)).await;

            // Verify engine is still functional (not deadlocked)
            let result2: Result<Result<(), crate::nostr_error::NostrError>, _> = timeout(
                Duration::from_millis(50),
                engine.unsubscribe(sub_id.to_string()),
            )
            .await;

            assert!(
                result2.is_ok(),
                "engine should still be responsive after slow storage query"
            );
        })
        .await;
}

// ============================================================================
// Test 2: Transport Connect Timeout
// ============================================================================

#[tokio::test]
async fn test_transport_connect_timeout() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let transport = Arc::new(HangingRelayTransport);
            let storage = Arc::new(HangingStorage);
            let signer = Arc::new(SlowSigner::with_delay(1));

            let (event_sink_tx, _event_sink_rx) =
                futures::channel::mpsc::channel::<(String, Vec<u8>)>(100);

            let engine = NostrEngine::new(
                transport.clone(),
                storage.clone(),
                signer.clone(),
                event_sink_tx,
            );

            // Create a publish request that will trigger relay connection
            let template = Template {
                kind: 1,
                content: "hello".to_string(),
                tags: vec![],
                created_at: 0,
            };

            // Use timeout to ensure the publish call doesn't hang
            // The publish message is sent to parser which will eventually need
            // to connect via the transport
            let result: Result<Result<(), crate::nostr_error::NostrError>, _> = timeout(
                Duration::from_millis(100),
                engine.publish(
                    "pub1".to_string(),
                    &template,
                    vec!["wss://r".to_string()],
                    vec![],
                ),
            )
            .await;

            // The publish call should complete (message sent to parser)
            assert!(result.is_ok(), "publish should complete within timeout");
            assert!(result.unwrap().is_ok(), "publish should succeed");

            // Give the system time to attempt connection
            tokio::time::sleep(Duration::from_millis(50)).await;

            // Verify engine is still functional (system didn't deadlock)
            let result2: Result<Result<(), crate::nostr_error::NostrError>, _> = timeout(
                Duration::from_millis(50),
                engine.subscribe("sub2".to_string(), vec![]),
            )
            .await;

            assert!(
                result2.is_ok(),
                "engine should remain responsive despite hanging transport"
            );
        })
        .await;
}

// ============================================================================
// Test 3: Signer Slow Response
// ============================================================================

#[tokio::test]
async fn test_signer_slow_response() {
    let local = LocalSet::new();
    local
        .run_until(async {
            // Create a signer that takes 5 seconds to sign
            let slow_signer = Arc::new(SlowSigner::with_delay(5000));
            let transport = Arc::new(HangingRelayTransport);
            let storage = Arc::new(HangingStorage);

            let (event_sink_tx, _event_sink_rx) =
                futures::channel::mpsc::channel::<(String, Vec<u8>)>(100);

            let engine = NostrEngine::new(
                transport.clone(),
                storage.clone(),
                slow_signer.clone(),
                event_sink_tx,
            );

            // Build a SignEvent message using Template
            let mut builder = flatbuffers::FlatBufferBuilder::new();
            
            // Create empty tags vector (tags is required field in Template)
            let tags_offset = builder.create_vector::<flatbuffers::WIPOffset<fb::StringVec>>(&[]);
            
            // Create strings first, before builder is borrowed
            let content_offset = builder.create_string("test content");
            
            // Create a minimal Template for signing
            let template_offset = fb::Template::create(
                &mut builder,
                &fb::TemplateArgs {
                    kind: 1,
                    content: Some(content_offset),
                    created_at: 1234567890,
                    tags: Some(tags_offset),
                },
            );
            
            let sign_event = fb::SignEvent::create(
                &mut builder,
                &fb::SignEventArgs {
                    template: Some(template_offset),
                },
            );
            let main_msg = fb::MainMessage::create(
                &mut builder,
                &fb::MainMessageArgs {
                    content_type: fb::MainContent::SignEvent,
                    content: Some(sign_event.as_union_value()),
                },
            );
            builder.finish(main_msg, None);
            let bytes = builder.finished_data().to_vec();

            // Send the sign request - this should not block the test
            // (the actual signing is done asynchronously by the crypto worker)
            let result: Result<Result<(), crate::nostr_error::NostrError>, _> = timeout(
                Duration::from_millis(100),
                engine.handle_message(&bytes),
            )
            .await;

            // The handle_message call should complete quickly (just sends to crypto worker)
            assert!(
                result.is_ok(),
                "handle_message should complete within timeout"
            );
            assert!(
                result.unwrap().is_ok(),
                "handle_message should succeed"
            );

            // Wait a bit but not the full 5 seconds to verify the system is responsive
            tokio::time::sleep(Duration::from_millis(50)).await;

            // Verify engine is still responsive (not blocked by slow signer)
            let result2: Result<Result<(), crate::nostr_error::NostrError>, _> = timeout(
                Duration::from_millis(50),
                engine.subscribe("sub_after_sign".to_string(), vec![]),
            )
            .await;

            assert!(
                result2.is_ok(),
                "engine should remain responsive despite slow signer"
            );
        })
        .await;
}

// ============================================================================
// Test 4: Channel Recv Timeout
// ============================================================================

#[tokio::test]
async fn test_channel_recv_timeout() {
    let local = LocalSet::new();
    local
        .run_until(async {
            // Create a WorkerChannel pair
            let (mut ch_a, ch_b) = TokioWorkerChannel::new_pair();

            // Drop the other side immediately to simulate closed channel
            drop(ch_b);

            // Try to recv with timeout - should return ChannelClosed or timeout
            let result: Result<Result<Vec<u8>, ChannelError>, _> = timeout(
                Duration::from_millis(100), 
                ch_a.recv()
            ).await;

            // The result should either be:
            // - Err(Elapsed) if timeout happens first
            // - Ok(Err(ChannelError::ChannelClosed)) if channel is detected as closed
            match result {
                Err(_elapsed) => {
                    // Timeout is acceptable - channel might not immediately detect closure
                }
                Ok(Err(e)) => {
                    // Channel error should be ChannelClosed
                    let err_str = format!("{:?}", e);
                    assert!(
                        err_str.contains("ChannelClosed") || err_str.contains("channel closed"),
                        "Expected ChannelClosed error, got: {:?}",
                        e
                    );
                }
                Ok(Ok(_)) => {
                    panic!("Should not receive data on closed channel");
                }
            }
        })
        .await;
}
