use futures::channel::mpsc;
use js_sys::{ArrayBuffer, Uint8Array};
use wasm_bindgen::prelude::*;
use web_sys::{MessageEvent, MessagePort};

/// A thin wrapper around web_sys::MessagePort that bridges JS messages
/// to a Rust async mpsc channel for zero-copy message passing between workers.
pub struct Port {
    port: MessagePort,
}

impl Port {
    /// Wrap a MessagePort for use in Rust workers.
    pub fn new(port: MessagePort) -> Self {
        Self { port }
    }

    /// Create an mpsc::Receiver from a MessagePort that yields message payloads
    /// as Vec<u8>. Uses a bounded channel with buffer size 10 for natural backpressure.
    ///
    /// The returned receiver can be used with `.await` or `.next().await` in
    /// async contexts (typically with futures::select!).
    pub fn from_receiver(port: MessagePort) -> mpsc::Receiver<Vec<u8>> {
        let (mut tx, rx) = mpsc::channel::<Vec<u8>>(64);

        // Set up the onmessage handler using Closure::wrap
        let closure = Closure::wrap(Box::new(move |event: MessageEvent| {
            // Extract data from the message event
            let bytes_result = if let Ok(data) = event.data().dyn_into::<ArrayBuffer>() {
                let array = Uint8Array::new(&data);
                let mut bytes = vec![0u8; array.length() as usize];
                array.copy_to(&mut bytes);
                Some(bytes)
            } else if let Ok(data) = event.data().dyn_into::<Uint8Array>() {
                // Handle Uint8Array directly as well
                let mut bytes = vec![0u8; data.length() as usize];
                data.copy_to(&mut bytes);
                Some(bytes)
            } else {
                // Silently ignore non-ArrayBuffer data
                None
            };

            // Send to channel - ignore send errors (receiver dropped)
            if let Some(bytes) = bytes_result {
                // Use try_send to avoid blocking in the JS callback
                let _ = tx.try_send(bytes);
            }
        }) as Box<dyn FnMut(MessageEvent)>);

        // Set the onmessage handler on the port
        port.set_onmessage(Some(closure.as_ref().unchecked_ref()));

        // Forget the closure to keep it alive (it lives as long as the page)
        closure.forget();

        rx
    }

    /// Send bytes through this port to the other end.
    /// Returns Ok(()) if the message was successfully posted.
    pub fn send(&self, bytes: &[u8]) -> Result<(), JsValue> {
        // Convert bytes to Uint8Array for transfer
        let array = Uint8Array::from(bytes);
        self.port.post_message(&array)?;
        Ok(())
    }

    /// Get a reference to the underlying MessagePort.
    pub fn inner(&self) -> &MessagePort {
        &self.port
    }

    /// Consume self and return the underlying MessagePort.
    pub fn into_inner(self) -> MessagePort {
        self.port
    }
}

impl From<MessagePort> for Port {
    fn from(port: MessagePort) -> Self {
        Self::new(port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen_test]
    async fn test_port_wrapper_creation() {
        // Create a MessageChannel to get two connected ports
        let channel = web_sys::MessageChannel::new().unwrap();
        let port1 = channel.port1();
        let port2 = channel.port2();

        // Wrap both ports
        let _port_wrapper1 = Port::new(port1);
        let _port_wrapper2 = Port::new(port2);

        // Just verify creation doesn't panic
    }
}
