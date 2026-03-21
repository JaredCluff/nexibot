//! Tool configurations: Search, Fetch, Filesystem, and Execute.

use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}

fn default_search_priority() -> Vec<String> {
    vec![
        "brave".to_string(),
        "tavily".to_string(),
        "browser".to_string(),
    ]
}
fn default_search_result_count() -> u32 {
    5
}

fn default_blocked_domains() -> Vec<String> {
    vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "0.0.0.0".to_string(),
        "169.254.169.254".to_string(),
        "[::1]".to_string(),
    ]
}
fn default_max_response_bytes() -> usize {
    1_048_576
} // 1MB
fn default_fetch_timeout() -> u64 {
    30_000
}

fn default_blocked_paths() -> Vec<String> {
    #[cfg(windows)]
    {
        vec![
            r"C:\Windows".to_string(),
            r"C:\Windows\System32".to_string(),
            r"C:\Program Files".to_string(),
            r"C:\Program Files (x86)".to_string(),
            r"C:\ProgramData".to_string(),
        ]
    }
    #[cfg(target_os = "macos")]
    {
        let mut paths = vec![
            "/etc".to_string(),
            "/System".to_string(),
            "/usr".to_string(),
            "/var".to_string(),
            "/bin".to_string(),
            "/sbin".to_string(),
        ];
        if let Some(home) = dirs::home_dir() {
            // Block NexiBot config/auth directories — contain API keys, tokens, secrets
            paths.push(home.join("Library/Application Support/ai.nexibot.desktop").to_string_lossy().to_string());
        }
        paths
    }
    #[cfg(target_os = "linux")]
    {
        let mut paths = vec![
            "/etc".to_string(),
            "/usr".to_string(),
            "/var".to_string(),
            "/bin".to_string(),
            "/sbin".to_string(),
        ];
        if let Some(home) = dirs::home_dir() {
            paths.push(home.join(".config/nexibot").to_string_lossy().to_string());
        }
        paths
    }
}
fn default_max_read_bytes() -> usize {
    1_048_576
} // 1MB
fn default_max_write_bytes() -> usize {
    10_485_760
} // 10MB

fn default_blocked_commands() -> Vec<String> {
    #[cfg(windows)]
    {
        vec![
            "format".to_string(),
            "del /s /q C:\\".to_string(),
            "rd /s /q C:\\".to_string(),
            "Remove-Item -Recurse -Force C:\\".to_string(),
            "reg delete".to_string(),
            "bcdedit".to_string(),
        ]
    }
    #[cfg(not(windows))]
    {
        vec![
            "rm -rf /".to_string(),
            "mkfs".to_string(),
            "dd if=".to_string(),
            ":(){ :|:& };:".to_string(),
            "> /dev/sda".to_string(),
        ]
    }
}
fn default_execute_timeout() -> u64 {
    30_000
}
fn default_max_output_bytes() -> usize {
    1_048_576
} // 1MB

/// Web search configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchConfig {
    /// Brave Search API key
    #[serde(default)]
    pub brave_api_key: Option<String>,
    /// Tavily API key
    #[serde(default)]
    pub tavily_api_key: Option<String>,
    /// Provider priority order (default: ["brave", "tavily", "browser"])
    #[serde(default = "default_search_priority")]
    pub search_priority: Vec<String>,
    /// Default number of results to return
    #[serde(default = "default_search_result_count")]
    pub default_result_count: u32,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            brave_api_key: None,
            tavily_api_key: None,
            search_priority: default_search_priority(),
            default_result_count: default_search_result_count(),
        }
    }
}

/// HTTP fetch tool configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchConfig {
    /// Whether the fetch tool is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Allowed domains (empty = allow all external)
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    /// Blocked domains (safety defaults)
    #[serde(default = "default_blocked_domains")]
    pub blocked_domains: Vec<String>,
    /// Maximum response body size in bytes (default: 1MB)
    #[serde(default = "default_max_response_bytes")]
    pub max_response_bytes: usize,
    /// Default request timeout in milliseconds
    #[serde(default = "default_fetch_timeout")]
    pub default_timeout_ms: u64,
}

impl Default for FetchConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allowed_domains: Vec::new(),
            blocked_domains: default_blocked_domains(),
            max_response_bytes: default_max_response_bytes(),
            default_timeout_ms: default_fetch_timeout(),
        }
    }
}

/// Filesystem tool configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilesystemConfig {
    /// Whether the filesystem tool is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Allowed paths (empty = home directory at runtime)
    #[serde(default)]
    pub allowed_paths: Vec<String>,
    /// Blocked paths (safety defaults)
    #[serde(default = "default_blocked_paths")]
    pub blocked_paths: Vec<String>,
    /// Maximum file read size in bytes (default: 1MB)
    #[serde(default = "default_max_read_bytes")]
    pub max_read_bytes: usize,
    /// Maximum file write size in bytes (default: 10MB)
    #[serde(default = "default_max_write_bytes")]
    pub max_write_bytes: usize,
}

impl Default for FilesystemConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allowed_paths: Vec::new(),
            blocked_paths: default_blocked_paths(),
            max_read_bytes: default_max_read_bytes(),
            max_write_bytes: default_max_write_bytes(),
        }
    }
}

/// Code execution tool configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecuteConfig {
    /// Whether the execute tool is enabled (default: TRUE — gated by guardrails + approval)
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Allowed commands (empty = allow all when enabled)
    #[serde(default)]
    pub allowed_commands: Vec<String>,
    /// Blocked command patterns (safety defaults)
    #[serde(default = "default_blocked_commands")]
    pub blocked_commands: Vec<String>,
    /// Default command timeout in milliseconds
    #[serde(default = "default_execute_timeout")]
    pub default_timeout_ms: u64,
    /// Maximum output size in bytes (default: 1MB)
    #[serde(default = "default_max_output_bytes")]
    pub max_output_bytes: usize,
    /// Working directory for commands
    #[serde(default)]
    pub working_directory: Option<String>,
    /// Use Destructive Command Guard for safety checks
    #[serde(default = "default_true")]
    pub use_dcg: bool,
    /// Whether skills can trigger command execution at runtime (default: FALSE)
    /// When false, skill-initiated tool calls to nexibot_execute are blocked
    #[serde(default)]
    pub skill_runtime_exec_enabled: bool,
    /// Sandbox policy override for this tool (default: uses global sandbox.policy)
    #[serde(default)]
    pub sandbox_policy: Option<crate::sandbox::policy::SandboxPolicy>,
}

impl Default for ExecuteConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allowed_commands: Vec::new(),
            blocked_commands: default_blocked_commands(),
            default_timeout_ms: default_execute_timeout(),
            max_output_bytes: default_max_output_bytes(),
            working_directory: None,
            use_dcg: true,
            skill_runtime_exec_enabled: false,
            sandbox_policy: None,
        }
    }
}
