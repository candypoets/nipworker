use nipworker_core::traits::{RelayTransport, TransportError, TransportStatus};
use std::cell::RefCell;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::rc::Rc;
use futures::channel::mpsc;
use futures::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

struct WsHandle {
    write: mpsc::UnboundedSender<String>,
    close_tx: tokio::sync::mpsc::UnboundedSender<()>,
}

pub struct NativeTransport {
    connections: Rc<RefCell<HashMap<String, WsHandle>>>,
    message_callbacks: Rc<RefCell<HashMap<String, Box<dyn Fn(String)>>>>,
    status_callbacks: Rc<RefCell<HashMap<String, Box<dyn Fn(TransportStatus)>>>>,
}

impl NativeTransport {
    pub fn new() -> Self {
        Self {
            connections: Rc::new(RefCell::new(HashMap::new())),
            message_callbacks: Rc::new(RefCell::new(HashMap::new())),
            status_callbacks: Rc::new(RefCell::new(HashMap::new())),
        }
    }
}

/// Parse host and port from a WebSocket URL.
fn parse_host_port(url_str: &str) -> Result<(String, u16), TransportError> {
    let parsed = url::Url::parse(url_str)
        .map_err(|e| TransportError::Other(format!("Invalid URL: {}", e)))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| TransportError::Other("URL has no host".to_string()))?
        .to_string();
    let port = parsed
        .port_or_known_default()
        .ok_or_else(|| TransportError::Other("URL has no port".to_string()))?;
    Ok((host, port))
}

const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const TCP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);
const TLS_WS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

/// Establish a WebSocket connection with detailed per-step logging and timeouts.
async fn open_websocket(
    url: &str,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>,
    TransportError,
> {
    tracing::info!(url = %url, "=== STEP 1: attempting connect_async with {}s timeout ===", CONNECT_TIMEOUT.as_secs());

    // 1. Try the standard path first, with a timeout.
    match tokio::time::timeout(
        CONNECT_TIMEOUT,
        tokio_tungstenite::connect_async(url),
    ).await
    {
        Ok(Ok((ws_stream, _))) => {
            tracing::info!(url = %url, "=== STEP 1 SUCCESS: connect_async returned Ok ===");
            return Ok(ws_stream);
        }
        Ok(Err(first_err)) => {
            let io_kind = match &first_err {
                tokio_tungstenite::tungstenite::Error::Io(io_err) => Some(io_err.kind()),
                _ => None,
            };
            tracing::warn!(
                url = %url,
                error = %first_err,
                io_kind = ?io_kind,
                "=== STEP 1 FAILED: connect_async returned Err ==="
            );
        }
        Err(_) => {
            tracing::error!(url = %url, "=== STEP 1 FAILED: connect_async timed out after {}s ===", CONNECT_TIMEOUT.as_secs());
        }
    }

    // 2. Parse host and port.
    tracing::info!(url = %url, "=== STEP 2: parsing URL ===");
    let (host, port) = parse_host_port(url)?;
    tracing::info!(url = %url, host = %host, port = port, "=== STEP 2 DONE ===");

    // 3. Resolve addresses.
    tracing::info!(url = %url, "=== STEP 3: DNS lookup with timeout ===");
    let addrs: Vec<SocketAddr> = match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::net::lookup_host((host.as_str(), port)),
    ).await
    {
        Ok(Ok(iter)) => {
            let addrs: Vec<_> = iter.collect();
            tracing::info!(url = %url, ?addrs, "=== STEP 3 DONE: DNS resolved ===");
            addrs
        }
        Ok(Err(e)) => {
            tracing::error!(url = %url, error = %e, "=== STEP 3 FAILED: DNS lookup error ===");
            return Err(TransportError::Other(format!(
                "DNS lookup failed for {}: {}",
                url, e
            )));
        }
        Err(_) => {
            tracing::error!(url = %url, "=== STEP 3 FAILED: DNS lookup timed out ===");
            return Err(TransportError::Other(format!(
                "DNS lookup timed out for {}",
                url
            )));
        }
    };

    if addrs.is_empty() {
        tracing::error!(url = %url, "=== STEP 3 FAILED: no addresses resolved ===");
        return Err(TransportError::Other(format!(
            "No addresses resolved for {}",
            url
        )));
    }

    // 4. Sort: IPv4 first, then IPv6.
    let mut ordered: Vec<SocketAddr> = addrs.clone();
    ordered.sort_by_key(|a| if a.is_ipv4() { 0 } else { 1 });
    tracing::info!(url = %url, ?ordered, "=== STEP 4: address order ===");

    // 5. Build the WebSocket request (hostname is preserved for TLS SNI).
    tracing::info!(url = %url, "=== STEP 5: building WS request ===");
    let request = url
        .into_client_request()
        .map_err(|e| TransportError::Other(format!("Invalid WebSocket request: {}", e)))?;
    tracing::info!(url = %url, "=== STEP 5 DONE ===");

    // 6. Try each address: TCP connect → TLS + WS handshake.
    let mut last_err: Option<tokio_tungstenite::tungstenite::Error> = None;
    for (idx, addr) in ordered.iter().enumerate() {
        tracing::info!(url = %url, %addr, idx, "=== STEP 6.{idx}: attempting TCP connect with {}s timeout ===", TCP_TIMEOUT.as_secs());

        let tcp_result = tokio::time::timeout(TCP_TIMEOUT, TcpStream::connect(*addr)).await;
        match tcp_result {
            Ok(Ok(stream)) => {
                tracing::info!(url = %url, %addr, idx, "=== STEP 6.{idx} TCP SUCCESS: connected ===");

                tracing::info!(url = %url, %addr, idx, "=== STEP 6.{idx}: attempting TLS+WS handshake with {}s timeout ===", TLS_WS_TIMEOUT.as_secs());
                let ws_result = tokio::time::timeout(
                    TLS_WS_TIMEOUT,
                    tokio_tungstenite::client_async_tls_with_config(
                        request.clone(),
                        stream,
                        None::<WebSocketConfig>,
                        None,
                    ),
                ).await;

                match ws_result {
                    Ok(Ok((ws_stream, _))) => {
                        tracing::info!(url = %url, %addr, idx, "=== STEP 6.{idx} WS SUCCESS: WebSocket connected ===");
                        return Ok(ws_stream);
                    }
                    Ok(Err(e)) => {
                        tracing::warn!(
                            url = %url,
                            %addr,
                            idx,
                            error = %e,
                            "=== STEP 6.{idx} WS FAILED: TLS/WebSocket handshake error ==="
                        );
                        last_err = Some(e);
                    }
                    Err(_) => {
                        tracing::warn!(
                            url = %url,
                            %addr,
                            idx,
                            "=== STEP 6.{idx} WS TIMEOUT: TLS/WebSocket handshake timed out after {}s ===",
                            TLS_WS_TIMEOUT.as_secs()
                        );
                        last_err = Some(tokio_tungstenite::tungstenite::Error::Io(
                            std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                "TLS/WebSocket handshake timeout",
                            )
                        ));
                    }
                }
            }
            Ok(Err(e)) => {
                tracing::warn!(
                    url = %url,
                    %addr,
                    idx,
                    error = %e,
                    "=== STEP 6.{idx} TCP FAILED ==="
                );
                last_err = Some(tokio_tungstenite::tungstenite::Error::Io(e));
            }
            Err(_) => {
                tracing::warn!(
                    url = %url,
                    %addr,
                    idx,
                    "=== STEP 6.{idx} TCP TIMEOUT: connection timed out after {}s ===",
                    TCP_TIMEOUT.as_secs()
                );
                last_err = Some(tokio_tungstenite::tungstenite::Error::Io(
                    std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "TCP connection timeout",
                    )
                ));
            }
        }
    }

    Err(TransportError::Other(format!(
        "WebSocket connect failed for {}. Last error: {:?}",
        url, last_err
    )))
}

#[async_trait::async_trait(?Send)]
impl RelayTransport for NativeTransport {
    async fn connect(&self, url: &str) -> Result<(), TransportError> {
        use tokio_tungstenite::tungstenite::Message;

        let ws_stream = open_websocket(url).await?;

        let (mut write, mut read) = ws_stream.split();
        let (tx, mut rx) = mpsc::unbounded::<String>();
        let (close_tx, mut close_rx) = tokio::sync::mpsc::unbounded_channel::<()>();

        let url_writer = url.to_string();
        let url_reader = url.to_string();
        let status_cbs = self.status_callbacks.clone();
        let msg_cbs = self.message_callbacks.clone();
        let connections = self.connections.clone();

        // Writer task
        let status_cbs_writer = status_cbs.clone();
        tokio::task::spawn_local(async move {
            while let Some(msg) = rx.next().await {
                if write.send(Message::Text(msg)).await.is_err() {
                    break;
                }
            }
            connections.borrow_mut().remove(&url_writer);
            if let Some(cb) = status_cbs_writer.borrow().get(&url_writer) {
                cb(TransportStatus::Closed {
                    url: url_writer.clone(),
                });
            }
        });

        // Reader task
        let status_cbs_reader = status_cbs.clone();
        tokio::task::spawn_local(async move {
            loop {
                tokio::select! {
                    msg = read.next() => {
                        match msg {
                            Some(Ok(Message::Text(text))) => {
                                if let Some(cb) = msg_cbs.borrow().get(&url_reader) {
                                    cb(text);
                                }
                            }
                            Some(Ok(_)) => {}
                            Some(Err(_)) | None => break,
                        }
                    }
                    _ = close_rx.recv() => break,
                }
            }
            if let Some(cb) = status_cbs_reader.borrow().get(&url_reader) {
                cb(TransportStatus::Closed {
                    url: url_reader.clone(),
                });
            }
        });

        self.connections
            .borrow_mut()
            .insert(url.to_string(), WsHandle { write: tx, close_tx });
        if let Some(cb) = self.status_callbacks.borrow().get(url) {
            cb(TransportStatus::Connected {
                url: url.to_string(),
            });
        }

        Ok(())
    }

    fn disconnect(&self, url: &str) {
        if let Some(handle) = self.connections.borrow_mut().remove(url) {
            let _ = handle.close_tx.send(());
        }
        if let Some(cb) = self.status_callbacks.borrow().get(url) {
            cb(TransportStatus::Closed {
                url: url.to_string(),
            });
        }
    }

    async fn send(&self, url: &str, frame: String) -> Result<(), TransportError> {
        if let Some(handle) = self.connections.borrow().get(url) {
            handle
                .write
                .unbounded_send(frame)
                .map_err(|e| TransportError::Other(format!("Send failed: {}", e)))
        } else {
            Err(TransportError::Other(format!("Not connected to {}", url)))
        }
    }

    fn on_message(&self, url: &str, callback: Box<dyn Fn(String)>) {
        self.message_callbacks
            .borrow_mut()
            .insert(url.to_string(), callback);
    }

    fn on_status(&self, url: &str, callback: Box<dyn Fn(TransportStatus)>) {
        self.status_callbacks
            .borrow_mut()
            .insert(url.to_string(), callback);
    }
}
