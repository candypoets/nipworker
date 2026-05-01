use futures::channel::mpsc;
use futures::{SinkExt, StreamExt};
use nipworker_core::traits::{RelayTransport, TransportError, TransportStatus};
use std::cell::RefCell;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::rc::Rc;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::Connector;

struct WsHandle {
    write: mpsc::UnboundedSender<String>,
    close_tx: tokio::sync::mpsc::UnboundedSender<()>,
}

pub struct NativeTransport {
    connections: Rc<RefCell<HashMap<String, WsHandle>>>,
    message_callbacks: Rc<RefCell<HashMap<String, Box<dyn Fn(String)>>>>,
    status_callbacks: Rc<RefCell<HashMap<String, Box<dyn Fn(TransportStatus)>>>>,
    tls_config: Arc<rustls::ClientConfig>,
}

impl NativeTransport {
    pub fn new() -> Self {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut root_store = rustls::RootCertStore::empty();
        root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let tls_config = Arc::new(
            rustls::ClientConfig::builder()
                .with_root_certificates(root_store)
                .with_no_client_auth(),
        );

        Self {
            connections: Rc::new(RefCell::new(HashMap::new())),
            message_callbacks: Rc::new(RefCell::new(HashMap::new())),
            status_callbacks: Rc::new(RefCell::new(HashMap::new())),
            tls_config,
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

const TCP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);
const TLS_WS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

/// Establish a WebSocket connection with detailed per-step logging and timeouts.
async fn open_websocket(
    url: &str,
    tls_config: Arc<rustls::ClientConfig>,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>,
    TransportError,
> {
    // Use manual path with pre-built TLS config to avoid blocking the LocalSet
    // with repeated ClientConfig construction (expensive with webpki-roots).
    log::info!(
        "=== Using manual WS path with shared rustls config for {} ===",
        url
    );

    // 2. Parse host and port.
    log::info!("=== STEP 2: parsing URL {} ===", url);
    let (host, port) = parse_host_port(url)?;
    log::info!("=== STEP 2 DONE: host={}, port={} ===", host, port);

    // 3. Resolve addresses.
    log::info!("=== STEP 3: DNS lookup for {} with timeout ===", url);
    let addrs: Vec<SocketAddr> = match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::net::lookup_host((host.as_str(), port)),
    )
    .await
    {
        Ok(Ok(iter)) => {
            let addrs: Vec<_> = iter.collect();
            log::info!("=== STEP 3 DONE: DNS resolved {:?} ===", addrs);
            addrs
        }
        Ok(Err(e)) => {
            log::error!("=== STEP 3 FAILED: DNS lookup error for {}: {} ===", url, e);
            return Err(TransportError::Other(format!(
                "DNS lookup failed for {}: {}",
                url, e
            )));
        }
        Err(_) => {
            log::error!("=== STEP 3 FAILED: DNS lookup timed out for {} ===", url);
            return Err(TransportError::Other(format!(
                "DNS lookup timed out for {}",
                url
            )));
        }
    };

    if addrs.is_empty() {
        log::error!("=== STEP 3 FAILED: no addresses resolved for {} ===", url);
        return Err(TransportError::Other(format!(
            "No addresses resolved for {}",
            url
        )));
    }

    // 4. Sort: IPv4 first, then IPv6.
    let mut ordered: Vec<SocketAddr> = addrs.clone();
    ordered.sort_by_key(|a| if a.is_ipv4() { 0 } else { 1 });
    log::info!("=== STEP 4: address order for {}: {:?} ===", url, ordered);

    // 5. Build the WebSocket request (hostname is preserved for TLS SNI).
    log::info!("=== STEP 5: building WS request for {} ===", url);
    let request = url
        .into_client_request()
        .map_err(|e| TransportError::Other(format!("Invalid WebSocket request: {}", e)))?;
    log::info!("=== STEP 5 DONE for {} ===", url);

    // 6. Try each address: TCP connect → TLS + WS handshake.
    let mut last_err: Option<tokio_tungstenite::tungstenite::Error> = None;
    for (idx, addr) in ordered.iter().enumerate() {
        log::info!(
            "=== STEP 6.{}: attempting TCP connect to {} with {}s timeout ===",
            idx,
            addr,
            TCP_TIMEOUT.as_secs()
        );

        let tcp_result = tokio::time::timeout(TCP_TIMEOUT, TcpStream::connect(*addr)).await;
        match tcp_result {
            Ok(Ok(stream)) => {
                log::info!("=== STEP 6.{} TCP SUCCESS: connected to {} ===", idx, addr);

                log::info!(
                    "=== STEP 6.{}: attempting TLS+WS handshake to {} with {}s timeout ===",
                    idx,
                    addr,
                    TLS_WS_TIMEOUT.as_secs()
                );
                let ws_result = tokio::time::timeout(
                    TLS_WS_TIMEOUT,
                    tokio_tungstenite::client_async_tls_with_config(
                        request.clone(),
                        stream,
                        None::<WebSocketConfig>,
                        Some(Connector::Rustls(tls_config.clone())),
                    ),
                )
                .await;

                match ws_result {
                    Ok(Ok((ws_stream, _))) => {
                        log::info!(
                            "=== STEP 6.{} WS SUCCESS: WebSocket connected to {} ===",
                            idx,
                            addr
                        );
                        return Ok(ws_stream);
                    }
                    Ok(Err(e)) => {
                        log::warn!(
                            "=== STEP 6.{} WS FAILED: TLS/WebSocket handshake error to {}: {} ===",
                            idx,
                            addr,
                            e
                        );
                        last_err = Some(e);
                    }
                    Err(_) => {
                        log::warn!("=== STEP 6.{} WS TIMEOUT: TLS/WebSocket handshake to {} timed out after {}s ===", idx, addr, TLS_WS_TIMEOUT.as_secs());
                        last_err = Some(tokio_tungstenite::tungstenite::Error::Io(
                            std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                "TLS/WebSocket handshake timeout",
                            ),
                        ));
                    }
                }
            }
            Ok(Err(e)) => {
                log::warn!("=== STEP 6.{} TCP FAILED to {}: {} ===", idx, addr, e);
                last_err = Some(tokio_tungstenite::tungstenite::Error::Io(e));
            }
            Err(_) => {
                log::warn!(
                    "=== STEP 6.{} TCP TIMEOUT: connection to {} timed out after {}s ===",
                    idx,
                    addr,
                    TCP_TIMEOUT.as_secs()
                );
                last_err = Some(tokio_tungstenite::tungstenite::Error::Io(
                    std::io::Error::new(std::io::ErrorKind::TimedOut, "TCP connection timeout"),
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

        let ws_stream = open_websocket(url, self.tls_config.clone()).await?;

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

        self.connections.borrow_mut().insert(
            url.to_string(),
            WsHandle {
                write: tx,
                close_tx,
            },
        );
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
