/// Minimal platform-agnostic port abstraction for message passing.
#[cfg(not(target_arch = "wasm32"))]
pub trait Port: Send + Sync {
	fn send(&self, bytes: &[u8]) -> Result<(), String>;
}

#[cfg(target_arch = "wasm32")]
pub trait Port {
	fn send(&self, bytes: &[u8]) -> Result<(), String>;
}
