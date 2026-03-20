//! Admin dashboard for gateway monitoring.
//!
//! Provides a simple HTML dashboard showing server statistics,
//! active connections, and message throughput.
//!
//! All HTTP responses include hardened security headers (CSP, X-Frame-Options,
//! X-Content-Type-Options, X-XSS-Protection) to mitigate XSS, clickjacking,
//! and MIME-sniffing attacks.
#![allow(dead_code)]

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Security headers
// ---------------------------------------------------------------------------

/// Content-Security-Policy value applied to every admin response.
const CSP_HEADER: &str = "default-src 'self'; script-src 'none'; style-src 'unsafe-inline'; \
    img-src 'self'; connect-src 'self'; frame-ancestors 'none'";

/// Standard security headers that must be present on every HTTP response
/// served by the admin dashboard.
pub struct AdminResponseHeaders;

impl AdminResponseHeaders {
    /// Return the set of security headers as `(name, value)` pairs.
    ///
    /// Callers should apply these to every HTTP response from the admin handler.
    pub fn headers() -> Vec<(&'static str, &'static str)> {
        vec![
            ("Content-Security-Policy", CSP_HEADER),
            ("X-Content-Type-Options", "nosniff"),
            ("X-Frame-Options", "DENY"),
            ("X-XSS-Protection", "1; mode=block"),
        ]
    }
}

/// An HTTP response from the admin dashboard, bundling the body with required
/// security headers.
#[derive(Debug, Clone)]
pub struct AdminResponse {
    /// Response body (HTML or JSON).
    pub body: String,
    /// Content-Type header value.
    pub content_type: &'static str,
    /// Security headers that MUST be set on the HTTP response.
    pub headers: Vec<(&'static str, &'static str)>,
}

impl AdminResponse {
    /// Build an HTML admin response with all required security headers.
    pub fn html(body: String) -> Self {
        Self {
            body,
            content_type: "text/html; charset=utf-8",
            headers: AdminResponseHeaders::headers(),
        }
    }

    /// Build a JSON admin response with all required security headers.
    pub fn json(body: String) -> Self {
        Self {
            body,
            content_type: "application/json",
            headers: AdminResponseHeaders::headers(),
        }
    }
}

/// Snapshot of the gateway's overall health and statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminDashboard {
    /// When the gateway server was started.
    pub server_start_time: DateTime<Utc>,
    /// Total connections since startup (including closed ones).
    pub total_connections: u64,
    /// Currently active WebSocket connections.
    pub active_connections: usize,
    /// Total messages routed through the gateway.
    pub total_messages_processed: u64,
    /// Seconds since the server started.
    pub uptime_seconds: u64,
    /// NexiBot version string.
    pub version: String,
}

/// Information about a single active connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionInfo {
    /// Session identifier.
    pub session_id: String,
    /// Authenticated user identifier.
    pub user_id: String,
    /// When the connection was established.
    pub connected_at: DateTime<Utc>,
    /// Number of messages sent by this connection.
    pub messages_sent: u64,
    /// Timestamp of the most recent activity.
    pub last_activity: DateTime<Utc>,
    /// Remote address (IP:port) of the client.
    pub remote_addr: String,
}

/// Tracks gateway-wide admin statistics.
///
/// # Security Note
///
/// The admin dashboard exposes operational metrics (connection counts,
/// uptime, remote addresses). Callers **must** gate access behind
/// gateway authentication before serving dashboard data to clients.
/// Unauthenticated access would leak internal network topology and
/// usage patterns (CWE-200).
#[derive(Debug)]
pub struct GatewayAdmin {
    start_time: DateTime<Utc>,
    total_connections: u64,
    total_messages: u64,
}

impl GatewayAdmin {
    /// Create a new admin tracker. Records the current time as startup.
    pub fn new() -> Self {
        Self {
            start_time: Utc::now(),
            total_connections: 0,
            total_messages: 0,
        }
    }

    /// Record that a new connection was established.
    pub fn record_connection(&mut self) {
        self.total_connections += 1;
    }

    /// Record that a message was processed.
    pub fn record_message(&mut self) {
        self.total_messages += 1;
    }

    /// Total connections recorded since startup.
    pub fn total_connections(&self) -> u64 {
        self.total_connections
    }

    /// Total messages recorded since startup.
    pub fn total_messages(&self) -> u64 {
        self.total_messages
    }

    /// Build a dashboard snapshot.
    pub fn get_dashboard(&self, active_connections: usize) -> AdminDashboard {
        let now = Utc::now();
        let uptime = (now - self.start_time).num_seconds().max(0) as u64;

        AdminDashboard {
            server_start_time: self.start_time,
            total_connections: self.total_connections,
            active_connections,
            total_messages_processed: self.total_messages,
            uptime_seconds: uptime,
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    /// Build a complete admin HTTP response with security headers.
    ///
    /// This is the primary entry point for serving the admin dashboard.
    /// The returned [`AdminResponse`] includes CSP, X-Frame-Options,
    /// X-Content-Type-Options, and X-XSS-Protection headers.
    pub fn build_admin_response(
        &self,
        active_connections: usize,
        connections: &[ConnectionInfo],
    ) -> AdminResponse {
        let html = self.render_html_dashboard(active_connections, connections);
        AdminResponse::html(html)
    }

    /// Build a JSON admin response with security headers.
    pub fn build_json_response(&self, active_connections: usize) -> AdminResponse {
        let dashboard = self.get_dashboard(active_connections);
        let json = serde_json::to_string_pretty(&dashboard).unwrap_or_default();
        AdminResponse::json(json)
    }

    /// Render a self-contained HTML dashboard page.
    ///
    /// Includes an auto-refresh `<meta>` tag that reloads every 10 seconds.
    pub fn render_html_dashboard(
        &self,
        active_connections: usize,
        connections: &[ConnectionInfo],
    ) -> String {
        let dashboard = self.get_dashboard(active_connections);

        let mut connections_html = String::new();
        for conn in connections {
            connections_html.push_str(&format!(
                "<tr>\
                    <td>{}</td>\
                    <td>{}</td>\
                    <td>{}</td>\
                    <td>{}</td>\
                    <td>{}</td>\
                    <td>{}</td>\
                </tr>",
                html_escape(&conn.session_id),
                html_escape(&conn.user_id),
                conn.connected_at.format("%Y-%m-%d %H:%M:%S UTC"),
                conn.messages_sent,
                conn.last_activity.format("%H:%M:%S UTC"),
                html_escape(&conn.remote_addr),
            ));
        }

        let uptime_display = format_uptime(dashboard.uptime_seconds);

        format!(
            r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="utf-8">
    <meta http-equiv="refresh" content="10">
    <title>NexiBot Gateway Admin</title>
    <style>
        body {{ font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; margin: 2rem; background: #f5f5f7; color: #1d1d1f; }}
        h1 {{ color: #0071e3; }}
        table {{ border-collapse: collapse; width: 100%; margin: 1rem 0; }}
        th, td {{ border: 1px solid #d2d2d7; padding: 0.5rem 1rem; text-align: left; }}
        th {{ background: #0071e3; color: white; }}
        tr:nth-child(even) {{ background: #fafafa; }}
        .stats {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(200px, 1fr)); gap: 1rem; margin: 1rem 0; }}
        .stat-card {{ background: white; border-radius: 12px; padding: 1.5rem; box-shadow: 0 1px 3px rgba(0,0,0,0.1); }}
        .stat-card .value {{ font-size: 2rem; font-weight: 700; color: #0071e3; }}
        .stat-card .label {{ color: #6e6e73; font-size: 0.875rem; }}
    </style>
</head>
<body>
    <h1>NexiBot Gateway Dashboard</h1>
    <p>Version {version} &mdash; Uptime: {uptime}</p>
    <div class="stats">
        <div class="stat-card">
            <div class="value">{active}</div>
            <div class="label">Active Connections</div>
        </div>
        <div class="stat-card">
            <div class="value">{total_conn}</div>
            <div class="label">Total Connections</div>
        </div>
        <div class="stat-card">
            <div class="value">{total_msg}</div>
            <div class="label">Messages Processed</div>
        </div>
    </div>
    <h2>Active Connections</h2>
    <table>
        <thead>
            <tr>
                <th>Session</th>
                <th>User</th>
                <th>Connected At</th>
                <th>Messages</th>
                <th>Last Activity</th>
                <th>Remote Address</th>
            </tr>
        </thead>
        <tbody>
            {connections}
        </tbody>
    </table>
    <p style="color: #6e6e73; font-size: 0.75rem;">Auto-refreshes every 10 seconds.</p>
</body>
</html>"#,
            version = html_escape(&dashboard.version),
            uptime = uptime_display,
            active = dashboard.active_connections,
            total_conn = dashboard.total_connections,
            total_msg = dashboard.total_messages_processed,
            connections = connections_html,
        )
    }
}

/// Minimal HTML escaping for safe insertion into the dashboard.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Format seconds into a human-readable "Xd Xh Xm Xs" string.
fn format_uptime(seconds: u64) -> String {
    let days = seconds / 86400;
    let hours = (seconds % 86400) / 3600;
    let minutes = (seconds % 3600) / 60;
    let secs = seconds % 60;

    if days > 0 {
        format!("{}d {}h {}m {}s", days, hours, minutes, secs)
    } else if hours > 0 {
        format!("{}h {}m {}s", hours, minutes, secs)
    } else if minutes > 0 {
        format!("{}m {}s", minutes, secs)
    } else {
        format!("{}s", secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gateway_admin_new() {
        let admin = GatewayAdmin::new();
        assert_eq!(admin.total_connections(), 0);
        assert_eq!(admin.total_messages(), 0);
    }

    #[test]
    fn test_record_connection() {
        let mut admin = GatewayAdmin::new();
        admin.record_connection();
        admin.record_connection();
        assert_eq!(admin.total_connections(), 2);
    }

    #[test]
    fn test_record_message() {
        let mut admin = GatewayAdmin::new();
        admin.record_message();
        admin.record_message();
        admin.record_message();
        assert_eq!(admin.total_messages(), 3);
    }

    #[test]
    fn test_get_dashboard() {
        let admin = GatewayAdmin::new();
        let dash = admin.get_dashboard(5);
        assert_eq!(dash.active_connections, 5);
        assert_eq!(dash.total_connections, 0);
        assert_eq!(dash.total_messages_processed, 0);
        assert!(!dash.version.is_empty());
    }

    #[test]
    fn test_render_html_dashboard_contains_key_elements() {
        let mut admin = GatewayAdmin::new();
        admin.record_connection();
        admin.record_message();

        let connections = vec![ConnectionInfo {
            session_id: "sess-1".to_string(),
            user_id: "user-1".to_string(),
            connected_at: Utc::now(),
            messages_sent: 42,
            last_activity: Utc::now(),
            remote_addr: "127.0.0.1:54321".to_string(),
        }];

        let html = admin.render_html_dashboard(1, &connections);
        assert!(html.contains("NexiBot Gateway Dashboard"));
        assert!(html.contains("meta http-equiv=\"refresh\" content=\"10\""));
        assert!(html.contains("sess-1"));
        assert!(html.contains("user-1"));
        assert!(html.contains("127.0.0.1:54321"));
        assert!(html.contains("Active Connections"));
    }

    #[test]
    fn test_html_escape() {
        assert_eq!(html_escape("<script>"), "&lt;script&gt;");
        assert_eq!(html_escape("a&b"), "a&amp;b");
        assert_eq!(html_escape("\"hello\""), "&quot;hello&quot;");
    }

    #[test]
    fn test_format_uptime() {
        assert_eq!(format_uptime(0), "0s");
        assert_eq!(format_uptime(45), "45s");
        assert_eq!(format_uptime(125), "2m 5s");
        assert_eq!(format_uptime(3661), "1h 1m 1s");
        assert_eq!(format_uptime(90061), "1d 1h 1m 1s");
    }

    #[test]
    fn test_dashboard_serde_roundtrip() {
        let dash = AdminDashboard {
            server_start_time: Utc::now(),
            total_connections: 100,
            active_connections: 5,
            total_messages_processed: 1000,
            uptime_seconds: 3600,
            version: "1.0.0".to_string(),
        };
        let json = serde_json::to_string(&dash).unwrap();
        let deserialized: AdminDashboard = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.total_connections, 100);
        assert_eq!(deserialized.active_connections, 5);
    }

    #[test]
    fn test_connection_info_serde() {
        let info = ConnectionInfo {
            session_id: "s1".to_string(),
            user_id: "u1".to_string(),
            connected_at: Utc::now(),
            messages_sent: 10,
            last_activity: Utc::now(),
            remote_addr: "10.0.0.1:8080".to_string(),
        };
        let json = serde_json::to_string(&info).unwrap();
        let deserialized: ConnectionInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.session_id, "s1");
        assert_eq!(deserialized.messages_sent, 10);
    }

    // -----------------------------------------------------------------------
    // CSP / security header tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_admin_response_headers_includes_csp() {
        let headers = AdminResponseHeaders::headers();
        let csp = headers
            .iter()
            .find(|(k, _)| *k == "Content-Security-Policy");
        assert!(csp.is_some(), "CSP header must be present");
        let (_, value) = csp.unwrap();
        assert!(value.contains("default-src 'self'"));
        assert!(value.contains("script-src 'none'"));
        assert!(value.contains("frame-ancestors 'none'"));
    }

    #[test]
    fn test_admin_response_headers_includes_xfo() {
        let headers = AdminResponseHeaders::headers();
        let xfo = headers.iter().find(|(k, _)| *k == "X-Frame-Options");
        assert!(xfo.is_some(), "X-Frame-Options header must be present");
        assert_eq!(xfo.unwrap().1, "DENY");
    }

    #[test]
    fn test_admin_response_headers_includes_xcto() {
        let headers = AdminResponseHeaders::headers();
        let xcto = headers.iter().find(|(k, _)| *k == "X-Content-Type-Options");
        assert!(
            xcto.is_some(),
            "X-Content-Type-Options header must be present"
        );
        assert_eq!(xcto.unwrap().1, "nosniff");
    }

    #[test]
    fn test_admin_response_headers_includes_xxss() {
        let headers = AdminResponseHeaders::headers();
        let xxss = headers.iter().find(|(k, _)| *k == "X-XSS-Protection");
        assert!(xxss.is_some(), "X-XSS-Protection header must be present");
        assert_eq!(xxss.unwrap().1, "1; mode=block");
    }

    #[test]
    fn test_build_admin_response_html() {
        let admin = GatewayAdmin::new();
        let response = admin.build_admin_response(0, &[]);
        assert_eq!(response.content_type, "text/html; charset=utf-8");
        assert!(response.body.contains("NexiBot Gateway Dashboard"));
        assert_eq!(response.headers.len(), 4);
    }

    #[test]
    fn test_build_json_response() {
        let admin = GatewayAdmin::new();
        let response = admin.build_json_response(3);
        assert_eq!(response.content_type, "application/json");
        assert!(response.body.contains("active_connections"));
        assert_eq!(response.headers.len(), 4);
    }

    #[test]
    fn test_admin_response_headers_are_consistent() {
        let html_resp = AdminResponse::html("test".to_string());
        let json_resp = AdminResponse::json("{}".to_string());
        // Both response types must carry the same security headers
        assert_eq!(html_resp.headers, json_resp.headers);
    }
}
