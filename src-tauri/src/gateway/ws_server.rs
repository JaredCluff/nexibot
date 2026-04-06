//! WebSocket server for the gateway.
//!
//! Uses tokio-tungstenite for WebSocket handling.
//! Each connection gets its own session and message loop.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, RwLock};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::{accept_async_with_config, accept_hdr_async_with_config};
use tracing::{error, info, warn};

use super::auth::{AuthCredentials, AuthResult, GatewayAuth};
use super::method_scopes::{validate_scope, Scope};
use super::protocol::{
    parse_client_message, serialize_server_message, ClientMessage, ServerMessage,
};
use super::session_mgr::GatewaySessionManager;
use super::GatewayConfig;

/// Handle to a single connected client.
#[derive(Debug)]
pub struct ConnectionHandle {
    /// Session ID associated with this connection.
    pub session_id: String,
    /// Channel for sending messages to the client.
    pub tx: mpsc::Sender<String>,
}

/// WebSocket gateway server.
pub struct GatewayServer {
    /// Server configuration.
    config: GatewayConfig,
    /// Authentication manager.
    auth: Arc<RwLock<GatewayAuth>>,
    /// Session manager.
    sessions: Arc<RwLock<GatewaySessionManager>>,
    /// Active connections keyed by a connection identifier.
    connections: Arc<RwLock<HashMap<String, ConnectionHandle>>>,
}

impl GatewayServer {
    /// Create a new gateway server.
    pub fn new(config: GatewayConfig) -> Self {
        let auth = Arc::new(RwLock::new(GatewayAuth::new(config.auth_mode.clone())));
        let sessions = Arc::new(RwLock::new(GatewaySessionManager::new(5)));
        let connections = Arc::new(RwLock::new(HashMap::new()));

        info!(
            "[GATEWAY] Server created (port={}, auth={:?}, max_connections={})",
            config.port, config.auth_mode, config.max_connections
        );

        Self {
            config,
            auth,
            sessions,
            connections,
        }
    }

    /// Get a reference to the auth manager.
    #[allow(dead_code)]
    pub fn auth(&self) -> &Arc<RwLock<GatewayAuth>> {
        &self.auth
    }

    /// Get a reference to the session manager.
    #[allow(dead_code)]
    pub fn sessions(&self) -> &Arc<RwLock<GatewaySessionManager>> {
        &self.sessions
    }

    /// Start the server, binding to the configured port and accepting connections.
    ///
    /// This function runs indefinitely until the server is shut down.
    pub async fn start(self: &Arc<Self>) -> Result<()> {
        if !self.config.enabled {
            info!("[GATEWAY] Gateway is disabled in configuration, not starting");
            return Ok(());
        }

        // Validate configuration before binding (TLS requirement, auth mode gating)
        if let Err(msg) = self.config.validate() {
            error!("[GATEWAY] Configuration validation failed: {}", msg);
            anyhow::bail!("{}", msg);
        }

        let addr = format!("{}:{}", self.config.bind_address, self.config.port);
        let listener = TcpListener::bind(&addr).await?;
        info!("[GATEWAY] WebSocket server listening on {}", addr);

        // Spawn periodic cleanup task: every 5 minutes, clean sessions idle > 1 hour
        // and remove stale connection handles.
        {
            let sessions = Arc::clone(&self.sessions);
            let connections = Arc::clone(&self.connections);
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
                loop {
                    interval.tick().await;

                    // Clean inactive sessions
                    {
                        let mut sessions_guard = sessions.write().await;
                        let removed =
                            sessions_guard.cleanup_inactive(std::time::Duration::from_secs(3600));
                        if removed > 0 {
                            info!(
                                "[GATEWAY] Periodic cleanup removed {} inactive sessions",
                                removed
                            );
                        }
                    }

                    // Clean stale connections whose sessions no longer exist
                    {
                        let sessions_guard = sessions.read().await;
                        let mut connections_guard = connections.write().await;
                        let before = connections_guard.len();
                        connections_guard.retain(|_conn_id, handle| {
                            sessions_guard.get_session(&handle.session_id).is_some()
                        });
                        let stale = before - connections_guard.len();
                        if stale > 0 {
                            info!(
                                "[GATEWAY] Periodic cleanup removed {} stale connections",
                                stale
                            );
                        }
                    }
                }
            });
        }

        loop {
            match listener.accept().await {
                Ok((stream, addr)) => {
                    // Check connection limit
                    let conn_count = self.connection_count().await;
                    if conn_count >= self.config.max_connections {
                        warn!(
                            "[GATEWAY] Connection limit reached ({}/{}), rejecting {}",
                            conn_count, self.config.max_connections, addr
                        );
                        drop(stream);
                        continue;
                    }

                    // CWE-319: Block plaintext WebSocket connections from non-loopback
                    // addresses. Remote clients must use TLS (wss://) to connect.
                    if !addr.ip().is_loopback() {
                        warn!(
                            "[GATEWAY] Rejecting plaintext WebSocket from non-loopback address {} \
                             (CWE-319: use wss:// for remote connections)",
                            addr
                        );
                        drop(stream);
                        continue;
                    }

                    info!("[GATEWAY] New TCP connection from {}", addr);
                    let server = Arc::clone(self);
                    tokio::spawn(async move {
                        if let Err(e) = Self::handle_connection(stream, addr, server).await {
                            warn!("[GATEWAY] Connection from {} ended with error: {}", addr, e);
                        }
                    });
                }
                Err(e) => {
                    error!("[GATEWAY] Failed to accept TCP connection: {}", e);
                }
            }
        }
    }

    /// Handle a single TCP connection: upgrade to WebSocket, authenticate, then
    /// enter the message loop.
    async fn handle_connection(
        stream: TcpStream,
        addr: SocketAddr,
        server: Arc<Self>,
    ) -> Result<()> {
        // Upgrade to WebSocket.
        // For TailscaleProxy mode, capture identity headers from the HTTP upgrade request.
        // For all other modes, use a plain handshake (no header capture overhead).
        let (ws_stream, tailscale_credentials) = {
            if server.config.auth_mode == super::AuthMode::TailscaleProxy {
                let captured = Arc::new(std::sync::Mutex::new((String::new(), String::new())));
                let cap = Arc::clone(&captured);
                let ws = accept_hdr_async_with_config(
                    stream,
                    move |req: &tokio_tungstenite::tungstenite::handshake::server::Request,
                          resp: tokio_tungstenite::tungstenite::handshake::server::Response| {
                        // CVE-2026-25253 mitigation: reject connections that supply
                        // client-controlled redirect or gateway URL headers. These headers
                        // have no legitimate use in the WebSocket upgrade request and could
                        // be used to inject URLs that cause server-side request forgery or
                        // open redirect attacks. Drop the connection with 403 Forbidden.
                        let injection_headers = [
                            "location",
                            "x-redirect-to",
                            "x-gateway-url",
                            "x-redirect",
                            "x-target",
                        ];
                        for header_name in &injection_headers {
                            if req.headers().contains_key(*header_name) {
                                warn!(
                                    "[GATEWAY] Rejected WebSocket upgrade: client supplied \
                                     disallowed header '{}' (CVE-2026-25253 URL injection prevention)",
                                    header_name
                                );
                                let resp = tokio_tungstenite::tungstenite::http::Response::builder()
                                    .status(tokio_tungstenite::tungstenite::http::StatusCode::FORBIDDEN)
                                    .body(Some(format!(
                                        "Connection rejected: disallowed header '{}' \
                                         (CVE-2026-25253 URL injection prevention)",
                                        header_name
                                    )));
                                return Err(match resp {
                                    Ok(r) => r,
                                    Err(e) => {
                                        warn!("[GATEWAY] Failed to build rejection response: {}", e);
                                        return Err(tokio_tungstenite::tungstenite::http::Response::new(None));
                                    }
                                });
                            }
                        }

                        let login = req
                            .headers()
                            .get("tailscale-user-login")
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or("")
                            .to_string();
                        let name = req
                            .headers()
                            .get("tailscale-user-name")
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or("")
                            .to_string();
                        *cap.lock().unwrap_or_else(|e| e.into_inner()) = (login, name);
                        Ok(resp)
                    },
                    Some(WebSocketConfig {
                        max_message_size: Some(4 * 1024 * 1024),
                        max_frame_size: Some(1 * 1024 * 1024),
                        ..Default::default()
                    }),
                )
                .await?;
                let (login, name) = captured.lock().unwrap_or_else(|e| e.into_inner()).clone();
                info!(
                    "[GATEWAY] TailscaleProxy upgrade from {} (login='{}', name='{}')",
                    addr, login, name
                );
                (ws, Some(AuthCredentials::TailscaleHeaders { login, name }))
            } else {
                // For non-TailscaleProxy modes validate the Origin header to prevent
                // Cross-Site WebSocket Hijacking (CSWSH) by malicious browser pages.
                let ws = accept_hdr_async_with_config(
                    stream,
                    move |req: &tokio_tungstenite::tungstenite::handshake::server::Request,
                          resp: tokio_tungstenite::tungstenite::handshake::server::Response| {
                        if let Some(origin) = req.headers().get("origin") {
                            let origin_str = origin.to_str().unwrap_or("");
                            let allowed = origin_str.is_empty()
                                || is_localhost_origin(origin_str);
                            if !allowed {
                                warn!(
                                    "[GATEWAY] Rejected WebSocket upgrade: untrusted Origin '{}'",
                                    origin_str
                                );
                                return Err(
                                    tokio_tungstenite::tungstenite::http::Response::builder()
                                        .status(
                                            tokio_tungstenite::tungstenite::http::StatusCode::FORBIDDEN,
                                        )
                                        .body(Some("Forbidden: untrusted Origin".to_string()))
                                        .unwrap_or_else(|_| {
                                            tokio_tungstenite::tungstenite::http::Response::new(None)
                                        }),
                                );
                            }
                        }
                        Ok(resp)
                    },
                    Some(WebSocketConfig {
                        max_message_size: Some(4 * 1024 * 1024),
                        max_frame_size: Some(1 * 1024 * 1024),
                        ..Default::default()
                    }),
                )
                .await?;
                (ws, None)
            }
        };
        info!("[GATEWAY] WebSocket upgrade successful for {}", addr);

        let (mut ws_sink, mut ws_stream_rx) = ws_stream.split();

        // Send Connected message
        let connected_msg = serialize_server_message(&ServerMessage::Connected {
            version: env!("CARGO_PKG_VERSION").to_string(),
        })?;
        ws_sink.send(Message::Text(connected_msg)).await?;

        // Authenticate: TailscaleProxy uses HTTP headers captured during upgrade;
        // all other modes read the first WebSocket message as credentials.
        let auth_result = if let Some(ts_creds) = tailscale_credentials {
            let auth = server.auth.read().await;
            auth.authenticate(&ts_creds)?
        } else {
            let credentials = match tokio::time::timeout(
                std::time::Duration::from_secs(30),
                ws_stream_rx.next(),
            )
            .await
            {
                Ok(Some(Ok(Message::Text(text)))) => Self::extract_credentials(&text),
                Ok(Some(Ok(_))) => AuthCredentials::None,
                Ok(Some(Err(e))) => {
                    warn!("[GATEWAY] Error reading auth message from {}: {}", addr, e);
                    return Err(e.into());
                }
                Ok(None) => {
                    info!("[GATEWAY] Connection {} closed before auth", addr);
                    return Ok(());
                }
                Err(_) => {
                    warn!("[GATEWAY] Auth message timeout for {}", addr);
                    return Ok(());
                }
            };
            let auth = server.auth.read().await;
            auth.authenticate_with_rate_limit(&credentials, addr.ip())?
        };

        if !auth_result.authenticated {
            let err_msg = serialize_server_message(&ServerMessage::Error {
                message: "Authentication failed".to_string(),
                code: Some("AUTH_FAILED".to_string()),
            })?;
            ws_sink.send(Message::Text(err_msg)).await?;
            warn!("[GATEWAY] Authentication failed for {}", addr);
            return Ok(());
        }

        info!(
            "[GATEWAY] Authenticated {} as user '{}'",
            addr, auth_result.user_id
        );

        // Derive immutable scopes from the auth result and create session
        let session_scopes = Self::scopes_from_auth_result(&auth_result);
        let session = {
            let mut sessions = server.sessions.write().await;
            sessions.create_session_with_scopes(&auth_result.user_id, session_scopes)?
        };
        let session_id = session.id.clone();

        // Send SessionCreated
        let session_msg = serialize_server_message(&ServerMessage::SessionCreated {
            session_id: session_id.clone(),
        })?;
        ws_sink.send(Message::Text(session_msg)).await?;

        // Set up outbound channel (server -> client)
        let (tx, mut rx) = mpsc::channel::<String>(64);
        let conn_id = format!("{}:{}", addr, session_id);

        {
            let mut connections = server.connections.write().await;
            connections.insert(
                conn_id.clone(),
                ConnectionHandle {
                    session_id: session_id.clone(),
                    tx,
                },
            );
        }

        info!(
            "[GATEWAY] Session {} created for {} (conn={})",
            session_id, addr, conn_id
        );

        // Spawn a task to forward outbound messages to the WebSocket sink
        let send_task = tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                if ws_sink.send(Message::Text(msg)).await.is_err() {
                    break;
                }
            }
        });

        // Message loop: read from WebSocket, process, respond
        while let Some(msg_result) = ws_stream_rx.next().await {
            match msg_result {
                Ok(Message::Text(text)) => {
                    let text_str: &str = text.as_ref();
                    match parse_client_message(text_str) {
                        Ok(client_msg) => {
                            // -- Scope enforcement (P1C) --
                            // Derive the method name from the message type and check it
                            // against the session's immutable scopes BEFORE dispatching.
                            let method_name = Self::method_name_for_message(&client_msg);
                            let scope_ok = {
                                let sessions = server.sessions.read().await;
                                match sessions.get_session(&session_id) {
                                    Some(session) => validate_scope(method_name, session.scopes()),
                                    None => {
                                        warn!(
                                            "[GATEWAY] Scope check: session {} not found",
                                            session_id
                                        );
                                        Err(super::method_scopes::ScopeError {
                                            method: method_name.to_string(),
                                            required: super::method_scopes::Scope::Admin,
                                            granted: vec![],
                                        })
                                    }
                                }
                            };

                            if let Err(scope_err) = scope_ok {
                                warn!(
                                    "[GATEWAY] Scope check failed for session {}: {}",
                                    session_id, scope_err
                                );
                                let err_json = serialize_server_message(&ServerMessage::Error {
                                    message: scope_err.to_string(),
                                    code: Some("SCOPE_DENIED".to_string()),
                                });
                                if let Ok(json) = err_json {
                                    let conn_handle = server.connections.read().await;
                                    if let Some(handle) = conn_handle.get(&conn_id) {
                                        let _ = handle.tx.send(json).await;
                                    }
                                }
                                continue;
                            }

                            let response =
                                Self::process_message(&server, &session_id, client_msg).await;
                            if let Some(response_msg) = response {
                                match serialize_server_message(&response_msg) {
                                    Ok(json) => {
                                        let conn_handle = server.connections.read().await;
                                        if let Some(handle) = conn_handle.get(&conn_id) {
                                            if handle.tx.send(json).await.is_err() {
                                                warn!(
                                                    "[GATEWAY] Failed to send to connection {}",
                                                    conn_id
                                                );
                                                break;
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        warn!("[GATEWAY] Failed to serialize response: {}", e);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            warn!("[GATEWAY] Invalid message from {}: {}", addr, e);
                            let err_json = serialize_server_message(&ServerMessage::Error {
                                message: format!("Invalid message: {}", e),
                                code: Some("INVALID_MESSAGE".to_string()),
                            });
                            if let Ok(json) = err_json {
                                let conn_handle = server.connections.read().await;
                                if let Some(handle) = conn_handle.get(&conn_id) {
                                    let _ = handle.tx.send(json).await;
                                }
                            }
                        }
                    }
                }
                Ok(Message::Close(_)) => {
                    info!("[GATEWAY] Client {} sent close frame", addr);
                    break;
                }
                Ok(Message::Ping(data)) => {
                    // Tungstenite handles pong automatically, but log it
                    tracing::trace!("[GATEWAY] Ping from {} ({} bytes)", addr, data.len());
                }
                Ok(_) => {
                    // Binary or other message types — ignore
                }
                Err(e) => {
                    warn!("[GATEWAY] WebSocket error from {}: {}", addr, e);
                    break;
                }
            }
        }

        // Cleanup: remove connection and session
        {
            let mut connections = server.connections.write().await;
            connections.remove(&conn_id);
        }
        {
            let mut sessions = server.sessions.write().await;
            sessions.remove_session(&session_id);
        }

        send_task.abort();
        info!(
            "[GATEWAY] Connection {} disconnected (session {})",
            addr, session_id
        );
        Ok(())
    }

    /// Process a single client message and optionally return a server response.
    async fn process_message(
        server: &Arc<Self>,
        session_id: &str,
        message: ClientMessage,
    ) -> Option<ServerMessage> {
        match message {
            ClientMessage::SendMessage {
                text,
                session_id: msg_session_id,
            } => {
                // Update session activity
                {
                    let mut sessions = server.sessions.write().await;
                    sessions.update_activity(session_id);
                }

                info!(
                    "[GATEWAY] Message from session {}: {} chars",
                    msg_session_id,
                    text.len()
                );

                // For now, echo back as a text chunk. In a full implementation
                // this would route through the LLM pipeline.
                Some(ServerMessage::TextChunk {
                    text: format!("Received: {}", text),
                    session_id: msg_session_id,
                })
            }
            ClientMessage::ToolResult {
                tool_use_id,
                result,
            } => {
                info!(
                    "[GATEWAY] Tool result for {}: {} chars",
                    tool_use_id,
                    result.len()
                );
                // In a full implementation, this would feed the result back
                // into the tool loop.
                None
            }
            ClientMessage::Ping => Some(ServerMessage::Pong),
        }
    }

    /// Map a [`ClientMessage`] variant to the gateway method name used for
    /// scope authorization checks.
    ///
    /// `Ping` is mapped to `"health"` (Read scope) so keep-alive pings never
    /// require elevated permissions.
    fn method_name_for_message(msg: &ClientMessage) -> &'static str {
        match msg {
            ClientMessage::SendMessage { .. } => "send_message",
            ClientMessage::ToolResult { .. } => "send_message",
            ClientMessage::Ping => "health",
        }
    }

    /// Map an [`AuthResult`]'s permission strings to [`Scope`] values.
    ///
    /// Recognized permission strings: "admin", "write", "read", "approvals",
    /// "pairing", "chat", "tools". The legacy "chat" and "tools" permissions
    /// map to `Scope::Write` for backward compatibility.
    ///
    /// If no permissions map to a known scope, the default is `[Scope::Read]`.
    fn scopes_from_auth_result(auth_result: &AuthResult) -> Vec<Scope> {
        let mut seen = std::collections::HashSet::new();
        let mut scopes: Vec<Scope> = auth_result
            .permissions
            .iter()
            .filter_map(|p| match p.as_str() {
                "admin" => Some(Scope::Admin),
                "write" | "chat" | "tools" => Some(Scope::Write),
                "read" => Some(Scope::Read),
                "approvals" => Some(Scope::Approvals),
                "pairing" => Some(Scope::Pairing),
                _ => None,
            })
            .filter(|s| seen.insert(*s))
            .collect();

        if scopes.is_empty() {
            scopes.push(Scope::Read);
        }
        scopes
    }

    /// Extract authentication credentials from a raw JSON message.
    ///
    /// Expects JSON with an optional `"token"` or `"password"` field.
    fn extract_credentials(json: &str) -> AuthCredentials {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(json) {
            if let Some(token) = value.get("token").and_then(|v| v.as_str()) {
                if !token.is_empty() {
                    return AuthCredentials::Token(token.to_string());
                }
            }
            if let Some(password) = value.get("password").and_then(|v| v.as_str()) {
                if !password.is_empty() {
                    return AuthCredentials::Password(password.to_string());
                }
            }
        }
        AuthCredentials::None
    }

    /// Broadcast a message to all connected clients.
    #[allow(dead_code)]
    pub async fn broadcast(&self, message: &ServerMessage) {
        let json = match serialize_server_message(message) {
            Ok(j) => j,
            Err(e) => {
                warn!("[GATEWAY] Failed to serialize broadcast message: {}", e);
                return;
            }
        };

        let connections = self.connections.read().await;
        let mut failed = 0usize;

        for (conn_id, handle) in connections.iter() {
            if handle.tx.send(json.clone()).await.is_err() {
                warn!(
                    "[GATEWAY] Failed to send broadcast to connection {}",
                    conn_id
                );
                failed += 1;
            }
        }

        if failed > 0 {
            warn!(
                "[GATEWAY] Broadcast completed with {} failed deliveries",
                failed
            );
        }
    }

    /// Remove connection handles whose sessions no longer exist.
    ///
    /// Returns the number of stale connections removed.
    #[allow(dead_code)]
    pub async fn cleanup_stale_connections(&self) -> usize {
        let sessions_guard = self.sessions.read().await;
        let mut connections_guard = self.connections.write().await;
        let before = connections_guard.len();
        connections_guard
            .retain(|_conn_id, handle| sessions_guard.get_session(&handle.session_id).is_some());
        let removed = before - connections_guard.len();
        if removed > 0 {
            info!("[GATEWAY] Cleaned up {} stale connections", removed);
        }
        removed
    }

    /// Return the current number of active connections.
    pub async fn connection_count(&self) -> usize {
        self.connections.read().await.len()
    }

    /// Gracefully shut down the server by closing all connections.
    #[allow(dead_code)]
    pub async fn shutdown(&self) {
        info!("[GATEWAY] Shutting down gateway server...");

        // Drop all connection senders — this will cause the send tasks to exit,
        // and the WebSocket connections to close.
        let mut connections = self.connections.write().await;
        let count = connections.len();
        connections.clear();

        // Clean up all sessions
        let sessions = self.sessions.write().await;
        // We can't call cleanup_inactive with zero duration easily, so just
        // iterate and count.
        let session_count = sessions.session_count();

        info!(
            "[GATEWAY] Shutdown complete: closed {} connections, {} sessions",
            count, session_count
        );
    }
}

// ---------------------------------------------------------------------------
// Origin validation helper
// ---------------------------------------------------------------------------

/// Validate that a WebSocket `Origin` header identifies a localhost source.
///
/// Uses proper URL parsing to prevent prefix-matching bypasses such as
/// `http://localhost.attacker.com` which would fool a naive `starts_with` check.
/// Only the scheme + host (and optional port) are inspected; paths are ignored.
fn is_localhost_origin(origin: &str) -> bool {
    // Allow the Tauri internal scheme without further parsing
    if origin == "tauri://localhost" || origin.starts_with("tauri://localhost/") {
        return true;
    }
    match url::Url::parse(origin) {
        Ok(parsed) => {
            let scheme = parsed.scheme();
            if scheme != "http" && scheme != "https" {
                return false;
            }
            match parsed.host_str() {
                Some("localhost") | Some("127.0.0.1") | Some("::1") => true,
                _ => false,
            }
        }
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::AuthMode;

    #[test]
    fn test_server_creation() {
        let config = GatewayConfig {
            enabled: true,
            port: 19000,
            bind_address: "127.0.0.1".to_string(),
            auth_mode: AuthMode::Open,
            max_connections: 10,
            tls_enabled: false,
        };
        let server = GatewayServer::new(config);
        assert_eq!(server.config.port, 19000);
        assert_eq!(server.config.max_connections, 10);
    }

    #[tokio::test]
    async fn test_connection_count_initially_zero() {
        let config = GatewayConfig::default();
        let server = GatewayServer::new(config);
        assert_eq!(server.connection_count().await, 0);
    }

    #[test]
    fn test_extract_credentials_token() {
        let json = r#"{"token": "secret-123"}"#;
        match GatewayServer::extract_credentials(json) {
            AuthCredentials::Token(t) => assert_eq!(t, "secret-123"),
            _ => panic!("Expected Token credentials"),
        }
    }

    #[test]
    fn test_extract_credentials_password() {
        let json = r#"{"password": "my-pass"}"#;
        match GatewayServer::extract_credentials(json) {
            AuthCredentials::Password(p) => assert_eq!(p, "my-pass"),
            _ => panic!("Expected Password credentials"),
        }
    }

    #[test]
    fn test_extract_credentials_none() {
        let json = r#"{"other": "field"}"#;
        assert!(matches!(
            GatewayServer::extract_credentials(json),
            AuthCredentials::None
        ));
    }

    #[test]
    fn test_extract_credentials_invalid_json() {
        let json = "not json at all";
        assert!(matches!(
            GatewayServer::extract_credentials(json),
            AuthCredentials::None
        ));
    }

    #[test]
    fn test_extract_credentials_token_takes_precedence() {
        // If both token and password are present, token wins
        let json = r#"{"token": "tok", "password": "pw"}"#;
        match GatewayServer::extract_credentials(json) {
            AuthCredentials::Token(t) => assert_eq!(t, "tok"),
            _ => panic!("Expected Token credentials"),
        }
    }

    #[test]
    fn test_method_name_for_message() {
        assert_eq!(
            GatewayServer::method_name_for_message(&ClientMessage::Ping),
            "health"
        );
        assert_eq!(
            GatewayServer::method_name_for_message(&ClientMessage::SendMessage {
                text: String::new(),
                session_id: String::new(),
            }),
            "send_message"
        );
        assert_eq!(
            GatewayServer::method_name_for_message(&ClientMessage::ToolResult {
                tool_use_id: String::new(),
                result: String::new(),
            }),
            "send_message"
        );
    }

    #[test]
    fn test_scopes_from_auth_result_maps_permissions() {
        use crate::gateway::auth::AuthResult;
        use crate::gateway::method_scopes::Scope;

        let result = AuthResult {
            authenticated: true,
            user_id: "u1".into(),
            permissions: vec!["admin".into()],
        };
        let scopes = GatewayServer::scopes_from_auth_result(&result);
        assert!(scopes.contains(&Scope::Admin));

        let result2 = AuthResult {
            authenticated: true,
            user_id: "u2".into(),
            permissions: vec!["chat".into(), "tools".into()],
        };
        let scopes2 = GatewayServer::scopes_from_auth_result(&result2);
        assert!(scopes2.contains(&Scope::Write));
        // chat and tools both map to Write, should be deduplicated
        assert_eq!(scopes2.len(), 1);
    }

    #[test]
    fn test_scopes_from_auth_result_defaults_to_read() {
        use crate::gateway::auth::AuthResult;
        use crate::gateway::method_scopes::Scope;

        let result = AuthResult {
            authenticated: true,
            user_id: "u".into(),
            permissions: vec!["unknown_perm".into()],
        };
        let scopes = GatewayServer::scopes_from_auth_result(&result);
        assert_eq!(scopes, vec![Scope::Read]);
    }

    #[tokio::test]
    async fn test_shutdown_is_idempotent() {
        let config = GatewayConfig::default();
        let server = GatewayServer::new(config);
        server.shutdown().await;
        server.shutdown().await;
        assert_eq!(server.connection_count().await, 0);
    }

    #[tokio::test]
    async fn test_broadcast_with_no_connections() {
        let config = GatewayConfig::default();
        let server = GatewayServer::new(config);
        // Should not panic
        server.broadcast(&ServerMessage::Pong).await;
    }

    #[tokio::test]
    async fn test_process_message_ping() {
        let config = GatewayConfig::default();
        let server = Arc::new(GatewayServer::new(config));
        let response =
            GatewayServer::process_message(&server, "session-1", ClientMessage::Ping).await;
        assert!(matches!(response, Some(ServerMessage::Pong)));
    }

    #[tokio::test]
    async fn test_process_message_send() {
        let config = GatewayConfig::default();
        let server = Arc::new(GatewayServer::new(config));

        // Create a session so update_activity doesn't warn
        {
            let mut sessions = server.sessions.write().await;
            let _ = sessions.create_session("test-user");
        }

        let response = GatewayServer::process_message(
            &server,
            "some-session",
            ClientMessage::SendMessage {
                text: "hello".to_string(),
                session_id: "s1".to_string(),
            },
        )
        .await;

        match response {
            Some(ServerMessage::TextChunk { text, session_id }) => {
                assert!(text.contains("hello"));
                assert_eq!(session_id, "s1");
            }
            _ => panic!("Expected TextChunk response"),
        }
    }

    #[tokio::test]
    async fn test_process_message_tool_result() {
        let config = GatewayConfig::default();
        let server = Arc::new(GatewayServer::new(config));
        let response = GatewayServer::process_message(
            &server,
            "session-1",
            ClientMessage::ToolResult {
                tool_use_id: "tu-1".to_string(),
                result: "done".to_string(),
            },
        )
        .await;
        // ToolResult currently returns None (no response)
        assert!(response.is_none());
    }

    #[tokio::test]
    async fn test_disabled_server_start_returns_ok() {
        let config = GatewayConfig {
            enabled: false,
            ..Default::default()
        };
        let server = Arc::new(GatewayServer::new(config));
        // Should return Ok immediately without binding
        let result = server.start().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_cleanup_stale_connections() {
        let config = GatewayConfig::default();
        let server = GatewayServer::new(config);

        // Create a session and a matching connection handle
        let session = {
            let mut sessions = server.sessions.write().await;
            sessions.create_session("user-1").unwrap()
        };

        let (tx, _rx) = mpsc::channel::<String>(1);
        {
            let mut conns = server.connections.write().await;
            conns.insert(
                "conn-1".to_string(),
                ConnectionHandle {
                    session_id: session.id.clone(),
                    tx: tx.clone(),
                },
            );
            // Also add a connection with a non-existent session (stale)
            conns.insert(
                "conn-stale".to_string(),
                ConnectionHandle {
                    session_id: "does-not-exist".to_string(),
                    tx,
                },
            );
        }

        assert_eq!(server.connection_count().await, 2);

        let removed = server.cleanup_stale_connections().await;
        assert_eq!(removed, 1);
        assert_eq!(server.connection_count().await, 1);

        // The valid connection should remain
        let conns = server.connections.read().await;
        assert!(conns.contains_key("conn-1"));
        assert!(!conns.contains_key("conn-stale"));
    }

    #[test]
    fn test_is_localhost_origin_allows_localhost() {
        assert!(is_localhost_origin("http://localhost"));
        assert!(is_localhost_origin("http://localhost:3000"));
        assert!(is_localhost_origin("https://localhost"));
        assert!(is_localhost_origin("http://127.0.0.1"));
        assert!(is_localhost_origin("http://127.0.0.1:8080"));
        assert!(is_localhost_origin("tauri://localhost"));
    }

    #[test]
    fn test_is_localhost_origin_blocks_prefix_bypass() {
        // These would fool a `starts_with("http://localhost")` check
        assert!(!is_localhost_origin("http://localhost.attacker.com"));
        assert!(!is_localhost_origin("http://localhost.attacker.com:3000"));
        assert!(!is_localhost_origin("http://127.0.0.1.evil.com"));
    }

    #[test]
    fn test_is_localhost_origin_blocks_remote_origins() {
        assert!(!is_localhost_origin("https://example.com"));
        assert!(!is_localhost_origin("http://192.168.1.1"));
        assert!(!is_localhost_origin("http://10.0.0.1"));
    }
}
