use thiserror::Error;

#[derive(Error, Debug)]
pub enum ChannelError {
	#[error("send failed: {0}")]
	SendFailed(String),
	#[error("recv failed: {0}")]
	RecvFailed(String),
	#[error("channel closed")]
	ChannelClosed,
}

#[cfg(not(target_arch = "wasm32"))]
pub trait WorkerChannelSender: Send + Sync {
	fn send(&self, bytes: &[u8]) -> Result<(), ChannelError>;
}

#[cfg(target_arch = "wasm32")]
pub trait WorkerChannelSender {
	fn send(&self, bytes: &[u8]) -> Result<(), ChannelError>;
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait::async_trait]
pub trait WorkerChannel: Send {
	async fn recv(&mut self) -> Result<Vec<u8>, ChannelError>;
	async fn send(&self, bytes: &[u8]) -> Result<(), ChannelError>;
	fn clone_sender(&self) -> Box<dyn WorkerChannelSender>;
}

#[cfg(target_arch = "wasm32")]
#[async_trait::async_trait(?Send)]
pub trait WorkerChannel {
	async fn recv(&mut self) -> Result<Vec<u8>, ChannelError>;
	async fn send(&self, bytes: &[u8]) -> Result<(), ChannelError>;
	fn clone_sender(&self) -> Box<dyn WorkerChannelSender>;
}

// ============== Native Implementation ==============

#[cfg(not(target_arch = "wasm32"))]
pub struct TokioWorkerChannel {
	sender: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
	receiver: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl TokioWorkerChannel {
	pub fn new_pair() -> (Self, Self) {
		let (tx_a, rx_a) = tokio::sync::mpsc::unbounded_channel();
		let (tx_b, rx_b) = tokio::sync::mpsc::unbounded_channel();
		(
			Self {
				sender: tx_a,
				receiver: rx_b,
			},
			Self {
				sender: tx_b,
				receiver: rx_a,
			},
		)
	}
}

#[cfg(not(target_arch = "wasm32"))]
impl WorkerChannelSender for tokio::sync::mpsc::UnboundedSender<Vec<u8>> {
	fn send(&self, bytes: &[u8]) -> Result<(), ChannelError> {
		self.send(bytes.to_vec())
			.map_err(|_| ChannelError::ChannelClosed)
	}
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait::async_trait]
impl WorkerChannel for TokioWorkerChannel {
	async fn recv(&mut self) -> Result<Vec<u8>, ChannelError> {
		self.receiver
			.recv()
			.await
			.ok_or(ChannelError::ChannelClosed)
	}

	async fn send(&self, bytes: &[u8]) -> Result<(), ChannelError> {
		self.sender
			.send(bytes.to_vec())
			.map_err(|_| ChannelError::ChannelClosed)
	}

	fn clone_sender(&self) -> Box<dyn WorkerChannelSender> {
		Box::new(self.sender.clone())
	}
}

// ============== WASM Implementation ==============

#[cfg(target_arch = "wasm32")]
pub struct WasmWorkerChannel {
	port: web_sys::MessagePort,
	receiver: futures::channel::mpsc::UnboundedReceiver<Vec<u8>>,
}

#[cfg(target_arch = "wasm32")]
impl WasmWorkerChannel {
	pub fn new(port: web_sys::MessagePort) -> Self {
		use wasm_bindgen::JsCast;
		let (tx, rx) = futures::channel::mpsc::unbounded();
		let tx = std::rc::Rc::new(std::cell::RefCell::new(tx));

		let closure = wasm_bindgen::closure::Closure::wrap(Box::new(
			move |event: web_sys::MessageEvent| {
				let data = event.data();
				let bytes = if let Ok(arr) = data.clone().dyn_into::<js_sys::Uint8Array>() {
					let mut v = vec![0u8; arr.length() as usize];
					arr.copy_to(&mut v);
					v
				} else if let Ok(buf) = data.dyn_into::<js_sys::ArrayBuffer>() {
					let arr = js_sys::Uint8Array::new(&buf);
					let mut v = vec![0u8; arr.length() as usize];
					arr.copy_to(&mut v);
					v
				} else {
					return;
				};
				let _ = tx.borrow_mut().unbounded_send(bytes);
			},
		) as Box<dyn FnMut(_)>);

		port.set_onmessage(Some(closure.as_ref().unchecked_ref()));
		closure.forget();

		Self { port, receiver: rx }
	}
}

#[cfg(target_arch = "wasm32")]
struct WasmWorkerChannelSender {
	port: web_sys::MessagePort,
}

#[cfg(target_arch = "wasm32")]
impl WorkerChannelSender for WasmWorkerChannelSender {
	fn send(&self, bytes: &[u8]) -> Result<(), ChannelError> {
		let buffer = js_sys::ArrayBuffer::new(bytes.len() as u32);
		let array = js_sys::Uint8Array::new(&buffer);
		array.copy_from(bytes);

		let transfer = js_sys::Array::new();
		transfer.push(&buffer);

		self.port
			.post_message_with_transferable(&buffer, &transfer)
			.map_err(|e| ChannelError::SendFailed(format!("{:?}", e)))
	}
}

#[cfg(target_arch = "wasm32")]
#[async_trait::async_trait(?Send)]
impl WorkerChannel for WasmWorkerChannel {
	async fn recv(&mut self) -> Result<Vec<u8>, ChannelError> {
		use futures::StreamExt;
		self.receiver
			.next()
			.await
			.ok_or(ChannelError::ChannelClosed)
	}

	async fn send(&self, bytes: &[u8]) -> Result<(), ChannelError> {
		let buffer = js_sys::ArrayBuffer::new(bytes.len() as u32);
		let array = js_sys::Uint8Array::new(&buffer);
		array.copy_from(bytes);

		let transfer = js_sys::Array::new();
		transfer.push(&buffer);

		self.port
			.post_message_with_transferable(&buffer, &transfer)
			.map_err(|e| ChannelError::SendFailed(format!("{:?}", e)))
	}

	fn clone_sender(&self) -> Box<dyn WorkerChannelSender> {
		Box::new(WasmWorkerChannelSender {
			port: self.port.clone(),
		})
	}
}

// ============== Port Adapter ==============

pub struct ChannelPort {
	sender: Box<dyn WorkerChannelSender>,
}

impl ChannelPort {
	pub fn new(sender: Box<dyn WorkerChannelSender>) -> Self {
		Self { sender }
	}
}

impl crate::port::Port for ChannelPort {
	fn send(&self, bytes: &[u8]) -> Result<(), String> {
		self.sender.send(bytes).map_err(|e| e.to_string())
	}
}


#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
	use super::*;

	#[tokio::test]
	async fn test_bidirectional_send_recv() {
		let (mut a, mut b) = TokioWorkerChannel::new_pair();
		a.send(b"hello a").await.unwrap();
		b.send(b"hello b").await.unwrap();
		assert_eq!(b.recv().await.unwrap(), b"hello a");
		assert_eq!(a.recv().await.unwrap(), b"hello b");
	}

	#[tokio::test]
	async fn test_clone_sender_survives_drop() {
		let (a, mut b) = TokioWorkerChannel::new_pair();
		let sender = a.clone_sender();
		drop(a);
		sender.send(b"cloned msg").unwrap();
		assert_eq!(b.recv().await.unwrap(), b"cloned msg");
	}

	#[tokio::test]
	async fn test_recv_returns_closed_when_dropped() {
		let (a, mut b) = TokioWorkerChannel::new_pair();
		drop(a);
		let result = b.recv().await;
		assert!(matches!(result, Err(ChannelError::ChannelClosed)));
	}

	#[tokio::test]
	async fn test_send_returns_closed_when_dropped() {
		let (a, b) = TokioWorkerChannel::new_pair();
		drop(b);
		let result = a.send(b"test").await;
		assert!(matches!(result, Err(ChannelError::ChannelClosed)));
	}
}
