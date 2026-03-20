//! Autonomous mode and per-capability access control configurations.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

fn default_true() -> bool {
    true
}
fn default_channel_denied_tools() -> Vec<String> {
    // Only deny filesystem by default. Execution is protected by its own layers
    // (guardrails, DCG, autonomous mode, config.execute.enabled) and should not
    // be blanket-denied per channel — that causes silent blocking on Telegram
    // and other headless channels with no way for the user to understand why.
    vec!["nexibot_filesystem".into()]
}

/// Autonomy level for individual capabilities in autonomous mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AutonomyLevel {
    /// Agent does it without asking
    Autonomous,
    /// Agent asks for explicit approval (blocks when no approval path exists)
    #[default]
    AskUser,
    /// Agent refuses, explains why if asked
    Blocked,
}

/// Per-channel tool access policy.
/// Controls which tools are denied on external channels (Telegram, Discord, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelToolPolicy {
    /// Tool names denied on this channel. Default: ["nexibot_filesystem"]
    #[serde(default = "default_channel_denied_tools")]
    pub denied_tools: Vec<String>,
    /// Tools explicitly allowed even if in denied_tools (override).
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    /// Whether admin users bypass denied_tools. Default: true
    #[serde(default = "default_true")]
    pub admin_bypass: bool,
}

impl Default for ChannelToolPolicy {
    fn default() -> Self {
        Self {
            denied_tools: default_channel_denied_tools(),
            allowed_tools: Vec::new(),
            admin_bypass: true,
        }
    }
}

impl ChannelToolPolicy {
    #[allow(dead_code)]
    pub fn allow_all() -> Self {
        Self {
            denied_tools: Vec::new(),
            allowed_tools: Vec::new(),
            admin_bypass: true,
        }
    }

    pub fn is_tool_denied(&self, tool_name: &str) -> bool {
        if self.allowed_tools.iter().any(|t| t == tool_name) {
            return false;
        }
        self.denied_tools.iter().any(|t| t == tool_name)
    }
}

/// Filesystem autonomy granularity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilesystemAutonomy {
    pub read: AutonomyLevel,
    pub write: AutonomyLevel,
    pub delete: AutonomyLevel,
}

impl Default for FilesystemAutonomy {
    fn default() -> Self {
        Self {
            read: AutonomyLevel::Autonomous,
            write: AutonomyLevel::AskUser,
            delete: AutonomyLevel::Blocked,
        }
    }
}

/// Execution autonomy granularity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecuteAutonomy {
    pub run_command: AutonomyLevel,
    pub run_python: AutonomyLevel,
    pub run_node: AutonomyLevel,
}

impl Default for ExecuteAutonomy {
    fn default() -> Self {
        Self {
            run_command: AutonomyLevel::AskUser,
            run_python: AutonomyLevel::AskUser,
            run_node: AutonomyLevel::AskUser,
        }
    }
}

/// Fetch autonomy granularity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchAutonomy {
    pub get_requests: AutonomyLevel,
    pub post_requests: AutonomyLevel,
}

impl Default for FetchAutonomy {
    fn default() -> Self {
        Self {
            get_requests: AutonomyLevel::Autonomous,
            post_requests: AutonomyLevel::AskUser,
        }
    }
}

/// Browser autonomy granularity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserAutonomy {
    pub navigate: AutonomyLevel,
    pub interact: AutonomyLevel,
}

impl Default for BrowserAutonomy {
    fn default() -> Self {
        Self {
            navigate: AutonomyLevel::AskUser,
            interact: AutonomyLevel::AskUser,
        }
    }
}

/// Generic capability autonomy (single level)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityAutonomy {
    pub level: AutonomyLevel,
}

impl Default for CapabilityAutonomy {
    fn default() -> Self {
        Self {
            level: AutonomyLevel::AskUser,
        }
    }
}

/// Autonomous mode configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AutonomousModeConfig {
    /// Master switch (default: false)
    #[serde(default)]
    pub enabled: bool,
    /// Filesystem read/write/delete granularity
    #[serde(default)]
    pub filesystem: FilesystemAutonomy,
    /// run_command/run_python/run_node
    #[serde(default)]
    pub execute: ExecuteAutonomy,
    /// get_requests/post_requests
    #[serde(default)]
    pub fetch: FetchAutonomy,
    /// navigate/interact
    #[serde(default)]
    pub browser: BrowserAutonomy,
    /// Computer use — single level
    #[serde(default)]
    pub computer_use: CapabilityAutonomy,
    /// Per-MCP-server autonomy
    #[serde(default)]
    pub mcp: HashMap<String, CapabilityAutonomy>,
    /// nexibot_settings tool
    #[serde(default)]
    pub settings_modification: CapabilityAutonomy,
    /// nexibot_memory tool
    #[serde(default)]
    pub memory_modification: CapabilityAutonomy,
    /// nexibot_soul tool
    #[serde(default)]
    pub soul_modification: CapabilityAutonomy,
}
