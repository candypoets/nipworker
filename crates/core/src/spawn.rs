#[cfg(target_arch = "wasm32")]
pub fn spawn_worker<F>(future: F)
where
	F: std::future::Future<Output = ()> + 'static,
{
	wasm_bindgen_futures::spawn_local(future);
}

#[cfg(not(target_arch = "wasm32"))]
pub fn spawn_worker<F>(future: F)
where
	F: std::future::Future<Output = ()> + 'static,
{
	tokio::task::spawn_local(future);
}
