/// Minimal platform-agnostic port abstraction for message passing.
pub trait Port: Send + Sync {
	fn send(&self, bytes: &[u8]) -> Result<(), String>;
}
