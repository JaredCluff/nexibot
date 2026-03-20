//! K2K Protocol Data Models

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ============================================================================
// Request Types
// ============================================================================

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct K2KQueryRequest {
    pub query: String,
    pub requesting_store: String,
    #[serde(default = "default_top_k")]
    pub top_k: usize,
    #[serde(default)]
    pub filters: Option<QueryFilters>,
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default)]
    pub target_stores: Option<Vec<String>>,
    /// Distributed trace ID for cross-node request correlation (Spec v1.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct QueryFilters {
    pub paths: Option<Vec<String>>,
    pub file_types: Option<Vec<String>>,
    pub max_file_size_bytes: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RegisterClientRequest {
    pub store_id: String,
    pub public_key_pem: String,
    pub device_id: String,
    pub key_algorithm: String,
    pub key_purpose: String,
}

// ============================================================================
// Response Types
// ============================================================================

#[derive(Debug, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub node_id: String,
    pub node_type: String,
    pub capabilities: Vec<String>,
    pub indexed_files: usize,
    pub uptime_seconds: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NodeInfo {
    pub node_id: String,
    pub node_name: String,
    pub node_type: String,
    pub version: String,
    pub public_key: String,
    pub federation_endpoint: String,
    pub allowed_paths: Vec<String>,
    pub blocked_patterns: Vec<String>,
    pub max_file_size: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RegisterClientResponse {
    pub registered: bool,
    pub key_id: String,
    pub store_id: String,
    pub expires_at: Option<String>,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct K2KQueryResponse {
    pub query_id: String,
    pub results: Vec<K2KResult>,
    pub total_results: usize,
    pub stores_queried: Vec<String>,
    pub query_time_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing_decision: Option<serde_json::Value>,
    /// Distributed trace ID echoed from the request (Spec v1.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct K2KResult {
    pub article_id: String,
    pub store_id: String,
    pub title: String,
    pub summary: String,
    pub content: String,
    pub confidence: f32,
    pub source_type: String,
    pub tags: Vec<String>,
    pub metadata: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<ResultProvenance>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ResultProvenance {
    pub store_id: String,
    pub store_type: String,
    pub original_rank: usize,
    pub rrf_score: f32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct K2KError {
    pub error: String,
    pub detail: String,
    pub status_code: u16,
}

// ============================================================================
// JWT Claims
// ============================================================================

#[derive(Debug, Deserialize, Serialize)]
pub struct K2KClaims {
    pub iss: String,          // Issuer: "kb:requesting_store"
    pub aud: String,          // Audience: "kb:requesting_store"
    pub source_kb_id: String, // Source KB ID (mirrors requesting_store)
    pub iat: i64,             // Issued at
    pub exp: i64,             // Expiration (5 minutes)
    pub jti: String,          // JWT ID for replay prevention
    pub transfer_id: String,
    pub client_id: String,
}

// ============================================================================
// Client Registry
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientKey {
    pub client_id: String,
    pub client_name: String,
    pub public_key_pem: String,
    pub registered_at: chrono::DateTime<chrono::Utc>,
}

// ============================================================================
// Helper Functions
// ============================================================================

fn default_top_k() -> usize {
    10
}

// ============================================================================
// Capability Advertisement (Phase 5)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CapabilityCategory {
    Knowledge,
    Tool,
    Skill,
    Compute,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCapability {
    pub id: String,
    pub name: String,
    pub category: CapabilityCategory,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<serde_json::Value>,
    pub version: String,
    /// Minimum K2K protocol version required to invoke this capability (Spec v1.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_protocol_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilitiesResponse {
    pub node_id: String,
    pub capabilities: Vec<AgentCapability>,
    pub protocol_version: String,
}

// ============================================================================
// Task Delegation (Phase 5)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRequest {
    pub capability_id: String,
    pub input: serde_json::Value,
    pub requesting_node_id: String,
    /// Client identity injected from the validated JWT `client_id` claim.
    /// Set by the HTTP handler before passing to `TaskQueue::submit`.
    #[serde(default)]
    pub client_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(default = "default_priority")]
    pub priority: String,
    /// Distributed trace ID for cross-node request correlation (Spec v1.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}

fn default_priority() -> String {
    "normal".to_string()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSubmitResponse {
    pub task_id: String,
    pub status: TaskStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStatusResponse {
    pub task_id: String,
    pub status: TaskStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<TaskResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    pub data: serde_json::Value,
    pub duration_ms: u64,
}

// ============================================================================
// SSE Events (Phase 5)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskEvent {
    pub task_id: String,
    /// Identity of the client that submitted this task.
    /// Used by SSE subscribers to filter events to their own tasks.
    pub client_id: String,
    pub event_type: TaskEventType,
    pub data: serde_json::Value,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskEventType {
    StatusChanged,
    Progress,
    Completed,
    Failed,
}

// ============================================================================
// Protocol Negotiation (Spec Improvement #1)
// ============================================================================

/// Protocol version handshake sent during initial connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolHandshake {
    /// Protocol version the client supports (e.g., "1.1").
    pub protocol_version: String,
    /// Minimum protocol version the client requires from the server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_protocol_version: Option<String>,
    /// Node ID of the initiating node.
    pub node_id: String,
    /// Human-readable node name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_name: Option<String>,
}

/// Protocol version handshake response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandshakeResponse {
    /// Protocol version the server will use for this session.
    pub negotiated_version: String,
    /// Server's node ID.
    pub node_id: String,
    /// Whether the server accepted the handshake.
    pub accepted: bool,
    /// Reason for rejection, if `accepted` is false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejection_reason: Option<String>,
}

// ============================================================================
// WAN Discovery (Spec Improvement #2)
// ============================================================================

/// DNS SRV record for WAN-based K2K node discovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsSrvRecord {
    /// Service name (e.g., "_k2k._tcp.example.com").
    pub service: String,
    /// Target hostname.
    pub target: String,
    /// Port number.
    pub port: u16,
    /// Priority (lower = preferred).
    #[serde(default)]
    pub priority: u16,
    /// Weight for load balancing among same-priority records.
    #[serde(default)]
    pub weight: u16,
}

/// Configuration for WAN-based node discovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WanDiscoveryConfig {
    /// DNS SRV domain to query (e.g., "_k2k._tcp.example.com").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dns_srv_domain: Option<String>,
    /// Bootstrap node URLs for initial discovery.
    #[serde(default)]
    pub bootstrap_nodes: Vec<String>,
    /// Whether to use WebFinger-style .well-known discovery.
    #[serde(default)]
    pub use_well_known: bool,
}

// ============================================================================
// K2K Error
// ============================================================================

impl K2KError {
    pub fn unauthorized(detail: impl Into<String>) -> Self {
        Self {
            error: "Unauthorized".into(),
            detail: detail.into(),
            status_code: 401,
        }
    }

    pub fn forbidden(detail: impl Into<String>) -> Self {
        Self {
            error: "Forbidden".into(),
            detail: detail.into(),
            status_code: 403,
        }
    }

    pub fn internal_error(detail: impl Into<String>) -> Self {
        Self {
            error: "Internal Server Error".into(),
            detail: detail.into(),
            status_code: 500,
        }
    }

    pub fn bad_request(detail: impl Into<String>) -> Self {
        Self {
            error: "Bad Request".into(),
            detail: detail.into(),
            status_code: 400,
        }
    }
}
