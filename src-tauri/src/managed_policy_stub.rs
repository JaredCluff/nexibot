//! Stub managed policy module when the `connect` feature is disabled.
//!
//! Provides the same public API as the real `managed_policy` module but does
//! nothing.  This avoids #[cfg] pollution throughout the rest of the codebase.

use std::sync::Arc;
use tokio::sync::RwLock;

use crate::config::NexiBotConfig;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PolicyStatus {
    pub enabled: bool,
    pub tier: Option<String>,
    pub policy_version: Option<String>,
    pub last_heartbeat_secs_ago: Option<u64>,
    pub voice_enabled: Option<bool>,
    pub voice_minutes_remaining: Option<i64>,
    pub max_channels: Option<usize>,
    pub autonomy_level: Option<String>,
    pub credits_remaining: Option<i64>,
    pub credits_monthly_limit: Option<i64>,
    pub k2k_federation_enabled: Option<bool>,
    pub computer_use_enabled: Option<bool>,
    pub scheduled_tasks_enabled: Option<bool>,
    pub expires_at: Option<i64>,
    pub restrictions: Vec<String>,
    pub kb_version: u32,
    pub kb_changed: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TierCapabilities {
    pub tier: Option<String>,
    pub voice_available: bool,
    pub multi_channel: bool,
    pub max_channels: Option<usize>,
    pub autonomous_mode: bool,
    pub k2k_federation: bool,
    pub computer_use: bool,
    pub scheduled_tasks: bool,
    pub browser_tool: bool,
    pub execute_tool: bool,
    pub filesystem_write: bool,
    pub credits_remaining: Option<i64>,
    pub dlp_required: bool,
}

pub struct ManagedPolicyManager;

impl ManagedPolicyManager {
    pub fn new(_config: Arc<RwLock<NexiBotConfig>>) -> Self {
        Self
    }

    pub async fn start(self: Arc<Self>) {}

    pub async fn get_status(&self) -> PolicyStatus {
        PolicyStatus {
            enabled: false,
            tier: None,
            policy_version: None,
            last_heartbeat_secs_ago: None,
            voice_enabled: None,
            voice_minutes_remaining: None,
            max_channels: None,
            autonomy_level: None,
            credits_remaining: None,
            credits_monthly_limit: None,
            k2k_federation_enabled: None,
            computer_use_enabled: None,
            scheduled_tasks_enabled: None,
            expires_at: None,
            restrictions: Vec::new(),
            kb_version: 0,
            kb_changed: false,
        }
    }

    pub async fn get_tier_capabilities(&self) -> TierCapabilities {
        TierCapabilities {
            tier: None,
            voice_available: true,
            multi_channel: true,
            max_channels: None,
            autonomous_mode: true,
            k2k_federation: true,
            computer_use: true,
            scheduled_tasks: true,
            browser_tool: true,
            execute_tool: true,
            filesystem_write: true,
            credits_remaining: None,
            dlp_required: false,
        }
    }

    pub async fn force_refresh(&self) -> anyhow::Result<()> {
        Ok(())
    }
}
