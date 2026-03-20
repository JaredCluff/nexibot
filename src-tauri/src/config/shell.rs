//! NexiGate gated shell configurations: policy, discovery, plugins, and shell settings.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

fn default_true() -> bool {
    true
}
fn default_max_concurrent_sessions() -> usize {
    20
}
fn default_min_secret_length() -> usize {
    20
}
pub fn default_shell_binary() -> String {
    #[cfg(windows)]
    {
        "powershell.exe".to_string()
    }
    #[cfg(not(windows))]
    {
        "/bin/bash".to_string()
    }
}
fn default_command_timeout_secs() -> u64 {
    30
}
fn default_gated_max_output_bytes() -> usize {
    102_400
}
fn default_max_audit_entries() -> usize {
    10_000
}
fn default_sentinel_prefix() -> String {
    "__NEXIGATE__".to_string()
}

/// Policy configuration for the gated shell.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatedShellPolicyConfig {
    /// Additional user-defined deny patterns (regex strings).
    #[serde(default)]
    pub deny_patterns: Vec<String>,
    /// Maximum number of concurrent shell sessions. Default: 20.
    #[serde(default = "default_max_concurrent_sessions")]
    pub max_concurrent_sessions: usize,
}

impl Default for GatedShellPolicyConfig {
    fn default() -> Self {
        Self {
            deny_patterns: Vec::new(),
            max_concurrent_sessions: default_max_concurrent_sessions(),
        }
    }
}

/// Configuration for dynamic secret discovery (Phase 7.5 of execute pipeline).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryConfig {
    /// Scan PTY output for known secret patterns. Default: true.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Run `printenv` after every command to catch `source .env` and `export` side-effects.
    /// Opt-in only — adds one sentinel round-trip per command. Default: false.
    #[serde(default)]
    pub track_env_changes: bool,
    /// Minimum secret length for env-diff heuristic. Default: 20.
    #[serde(default = "default_min_secret_length")]
    pub min_secret_length: usize,
    /// Additional user-defined regex patterns. Each entry: { name, pattern, format }.
    #[serde(default)]
    pub extra_patterns: Vec<ExtraDiscoveryPattern>,
}

/// A user-defined extra discovery pattern.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtraDiscoveryPattern {
    pub name: String,
    pub pattern: String,
    pub format: String,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            track_env_changes: false,
            min_secret_length: default_min_secret_length(),
            extra_patterns: Vec::new(),
        }
    }
}

/// Configuration for the signed security plugin system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginConfig {
    /// Enable the plugin system. Default: false.
    #[serde(default)]
    pub enabled: bool,
    /// Directory to load .rhai + .manifest.json plugins from.
    /// Default: ~/.config/nexibot/shell_plugins/
    #[serde(default)]
    pub plugin_dir: Option<PathBuf>,
    /// Trusted Ed25519 public keys (hex-encoded 32 bytes) for plugin signature verification.
    #[serde(default)]
    pub trusted_keys: Vec<String>,
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            plugin_dir: None,
            trusted_keys: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tmux interactive agent bridge config
// ---------------------------------------------------------------------------

fn default_tmux_poll_interval_ms() -> u64 {
    200
}
fn default_tmux_stable_ms() -> u64 {
    2_000
}
fn default_tmux_wait_timeout_ms() -> u64 {
    120_000
}
fn default_tmux_max_sessions() -> usize {
    20
}

/// A user-defined custom agent type with bespoke state-detection patterns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomAgentPattern {
    /// Agent type name (used as `agent_type` in start_session).
    pub name: String,
    /// Regex that signals a Ready (interactive prompt) state.
    #[serde(default)]
    pub ready: Option<String>,
    /// Regex that signals the agent is busy / Running.
    #[serde(default)]
    pub running: Option<String>,
    /// Regex that signals an Approval prompt.
    #[serde(default)]
    pub approval: Option<String>,
    /// Regex that signals an Error state.
    #[serde(default)]
    pub error: Option<String>,
}

/// Configuration for the tmux-based interactive agent bridge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TmuxConfig {
    /// Enable the tmux interactive agent bridge. Default: false.
    #[serde(default)]
    pub enabled: bool,
    /// Pane polling interval (ms). Default: 200.
    #[serde(default = "default_tmux_poll_interval_ms")]
    pub poll_interval_ms: u64,
    /// How long (ms) pane content must be stable before reporting `UnknownStable`.
    /// Default: 2000.
    #[serde(default = "default_tmux_stable_ms")]
    pub content_stable_ms: u64,
    /// Default timeout (ms) for `wait_for_state`. Default: 120000.
    #[serde(default = "default_tmux_wait_timeout_ms")]
    pub wait_timeout_ms: u64,
    /// Maximum concurrent interactive sessions. Default: 20.
    #[serde(default = "default_tmux_max_sessions")]
    pub max_sessions: usize,
    /// Additional user-defined agent type patterns.
    #[serde(default)]
    pub custom_agents: Vec<CustomAgentPattern>,
}

impl Default for TmuxConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            poll_interval_ms: default_tmux_poll_interval_ms(),
            content_stable_ms: default_tmux_stable_ms(),
            wait_timeout_ms: default_tmux_wait_timeout_ms(),
            max_sessions: default_tmux_max_sessions(),
            custom_agents: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Main GatedShellConfig
// ---------------------------------------------------------------------------

/// Configuration for NexiGate, the gated shell subsystem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatedShellConfig {
    /// Enable the gated shell. Default: false (safe opt-in).
    #[serde(default)]
    pub enabled: bool,
    /// Enable debug mode (keeps full raw PTY output in audit log). Default: false.
    #[serde(default)]
    pub debug_mode: bool,
    /// Record sessions to asciicast v2 files. Default: false.
    #[serde(default)]
    pub record_sessions: bool,
    /// Override directory for session recordings.
    #[serde(default)]
    pub recordings_dir: Option<PathBuf>,
    /// Shell binary to spawn. Default: "/bin/bash" (Unix) or "powershell.exe" (Windows).
    #[serde(default = "default_shell_binary")]
    pub shell_binary: String,
    /// Default command timeout in seconds. Default: 30.
    #[serde(default = "default_command_timeout_secs")]
    pub command_timeout_secs: u64,
    /// Maximum output bytes per command before truncation. Default: 102400 (100 KB).
    #[serde(default = "default_gated_max_output_bytes")]
    pub max_output_bytes: usize,
    /// Maximum audit entries kept per session. Default: 10000.
    #[serde(default = "default_max_audit_entries")]
    pub max_audit_entries: usize,
    /// Sentinel prefix for command completion detection. Default: "__NEXIGATE__".
    #[serde(default = "default_sentinel_prefix")]
    pub sentinel_prefix: String,
    /// Policy settings.
    #[serde(default)]
    pub policy: GatedShellPolicyConfig,
    /// Dynamic secret discovery settings.
    #[serde(default)]
    pub discovery: DiscoveryConfig,
    /// Signed security plugin settings.
    #[serde(default)]
    pub plugins: PluginConfig,
    /// Tmux interactive agent bridge settings.
    #[serde(default)]
    pub tmux: TmuxConfig,
}

impl Default for GatedShellConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            debug_mode: false,
            record_sessions: false,
            recordings_dir: None,
            shell_binary: default_shell_binary(),
            command_timeout_secs: default_command_timeout_secs(),
            max_output_bytes: default_gated_max_output_bytes(),
            max_audit_entries: default_max_audit_entries(),
            sentinel_prefix: default_sentinel_prefix(),
            policy: GatedShellPolicyConfig::default(),
            discovery: DiscoveryConfig::default(),
            plugins: PluginConfig::default(),
            tmux: TmuxConfig::default(),
        }
    }
}
