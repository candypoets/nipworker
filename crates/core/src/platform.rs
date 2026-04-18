//! Cross-platform time helpers for native and WASM targets.

/// Returns the current wall-clock time in milliseconds since the UNIX epoch.
pub fn now_millis() -> u64 {
	#[cfg(target_arch = "wasm32")]
	{
		js_sys::Date::now() as u64
	}

	#[cfg(not(target_arch = "wasm32"))]
	{
		std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap_or_default()
			.as_millis() as u64
	}
}

/// Sleeps for the given duration in milliseconds.
pub async fn sleep(duration_ms: u64) {
	#[cfg(target_arch = "wasm32")]
	{
		gloo_timers::future::TimeoutFuture::new(duration_ms as u32).await;
	}

	#[cfg(not(target_arch = "wasm32"))]
	{
		tokio::time::sleep(std::time::Duration::from_millis(duration_ms)).await;
	}
}
