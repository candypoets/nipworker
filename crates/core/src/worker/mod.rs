pub mod parser_worker;

#[cfg(not(target_arch = "wasm32"))]
pub mod connections_worker;

#[cfg(not(target_arch = "wasm32"))]
pub mod cache_worker;

#[cfg(not(target_arch = "wasm32"))]
pub mod crypto_worker;

#[cfg(target_arch = "wasm32")]
pub mod batch_buffer;
