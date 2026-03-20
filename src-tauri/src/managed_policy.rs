//! Knowledge Nexus Central Management — Managed Policy Manager
//!
//! When `managed_policy.enabled = true`, NexiBot:
//! 1. Registers with the KN server on startup (upsert by instance_id).
//! 2. Applies startup-cached policy immediately (closes unrestricted boot window).
//! 3. Fetches the active managed policy and applies security floors/ceilings locally.
//! 4. Sends periodic heartbeats.  If the server signals a policy update,
//!    NexiBot applies the inline policy (if provided) or re-fetches it.
//! 5. Honors `pending_command` from the server ("Disconnect", "Suspend", "FetchPolicy").
//!
//! **Security model — floors and ceilings, never overriding stricter local settings.**
//! The server specifies *minimum* security levels (floors) and *maximum* capability
//! ceilings.  If the local config is already stricter, it is left unchanged.

use anyhow::Result;
use nexibot_nexus::{HeartbeatRequest, InstanceRegistration, KnManagementClient, ManagedPolicy, PolicyCommand};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::config::NexiBotConfig;
use crate::guardrails::SecurityLevel;

// ---------------------------------------------------------------------------
// Policy status (exposed to Tauri commands)
// ---------------------------------------------------------------------------

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
    /// Personal KB version as last reported by the server.
    /// The frontend can watch this value across successive `get_managed_policy_status`
    /// calls to detect when a KB refresh has been signalled and display a stale-KB indicator.
    pub kb_version: u32,
    /// True if the most recent heartbeat response indicated that the personal KB
    /// has changed since NexiBot last confirmed sync.  Cleared on the next heartbeat
    /// once NexiBot has updated its cached kb_version.
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
    /// Whether Data Loss Prevention scanning is required (true = DLP enforced).
    /// Absent/None means DLP is required (fail-closed default).
    pub dlp_required: bool,
}

// ---------------------------------------------------------------------------
// Manager
// ---------------------------------------------------------------------------

pub struct ManagedPolicyManager {
    config: Arc<RwLock<NexiBotConfig>>,
    /// Policy version string held by this instance (for change detection).
    current_policy_version: Arc<RwLock<Option<String>>>,
    /// Last applied policy (for status queries).
    last_policy: Arc<RwLock<Option<ManagedPolicy>>>,
    /// Monotonic instant of the most recent successful heartbeat.
    last_heartbeat: Arc<RwLock<Option<Instant>>>,
    /// Personal KB version as last confirmed from the server.
    /// Sent on every heartbeat as `current_kb_version`.
    /// Updated when the server responds with `kb_version`.
    kb_version: Arc<RwLock<u32>>,
    /// Whether the most recent heartbeat indicated a KB change.
    /// Exposed via `get_status()` so the Tauri frontend can show a stale-KB indicator.
    kb_changed: Arc<RwLock<bool>>,
}

impl ManagedPolicyManager {
    pub fn new(config: Arc<RwLock<NexiBotConfig>>) -> Self {
        Self {
            config,
            current_policy_version: Arc::new(RwLock::new(None)),
            last_policy: Arc::new(RwLock::new(None)),
            last_heartbeat: Arc::new(RwLock::new(None)),
            kb_version: Arc::new(RwLock::new(0)),
            kb_changed: Arc::new(RwLock::new(false)),
        }
    }

    // -----------------------------------------------------------------------
    // Public API
    // -----------------------------------------------------------------------

    /// Bootstrap: register with KN, apply initial policy, start heartbeat loop.
    ///
    /// Safe to call unconditionally — returns immediately if `enabled = false`.
    pub async fn start(self: Arc<Self>) {
        let enabled = self.config.read().await.managed_policy.enabled;
        if !enabled {
            return;
        }

        let mgr = self.clone();
        tokio::spawn(async move {
            if let Err(e) = mgr.run().await {
                warn!("[MANAGED_POLICY] Startup failed: {}", e);
            }
        });
    }

    /// Return the current policy status (for Tauri command `get_managed_policy_status`).
    pub async fn get_status(&self) -> PolicyStatus {
        let enabled = self.config.read().await.managed_policy.enabled;
        let policy_version = self.current_policy_version.read().await.clone();
        let last_hb = self.last_heartbeat.read().await.map(|i| i.elapsed().as_secs());
        let policy = self.last_policy.read().await.clone();
        let kb_version = *self.kb_version.read().await;
        let kb_changed = *self.kb_changed.read().await;

        let mut restrictions: Vec<String> = Vec::new();

        let (tier, voice_enabled, voice_minutes_remaining, max_channels, autonomy_level,
             credits_remaining, credits_monthly_limit, k2k_federation_enabled,
             computer_use_enabled, scheduled_tasks_enabled, expires_at) = match &policy {
            Some(p) => {
                if p.voice_enabled == Some(false) {
                    restrictions.push("voice_disabled".into());
                }
                if let Some(max) = p.max_channels {
                    restrictions.push(format!("max_channels:{}", max));
                }
                if p.k2k_federation_enabled == Some(false) {
                    restrictions.push("k2k_disabled".into());
                }
                if p.computer_use_enabled == Some(false) {
                    restrictions.push("computer_use_disabled".into());
                }
                if p.scheduled_tasks_enabled == Some(false) {
                    restrictions.push("scheduled_tasks_disabled".into());
                }
                if let Some(ref tr) = p.tool_restrictions {
                    if tr.execute_allowed == Some(false) {
                        restrictions.push("execute_disabled".into());
                    }
                    if tr.browser_allowed == Some(false) {
                        restrictions.push("browser_disabled".into());
                    }
                    if tr.filesystem_write_allowed == Some(false) {
                        restrictions.push("filesystem_write_disabled".into());
                    }
                }
                (
                    p.tier.clone(),
                    p.voice_enabled,
                    p.voice_minutes_remaining,
                    p.max_channels,
                    p.autonomy_level.clone(),
                    p.credits_remaining,
                    p.credits_monthly_limit,
                    p.k2k_federation_enabled,
                    p.computer_use_enabled,
                    p.scheduled_tasks_enabled,
                    p.expires_at,
                )
            }
            None => (None, None, None, None, None, None, None, None, None, None, None),
        };

        PolicyStatus {
            enabled,
            tier,
            policy_version,
            last_heartbeat_secs_ago: last_hb,
            voice_enabled,
            voice_minutes_remaining,
            max_channels,
            autonomy_level,
            credits_remaining,
            credits_monthly_limit,
            k2k_federation_enabled,
            computer_use_enabled,
            scheduled_tasks_enabled,
            expires_at,
            restrictions,
            kb_version,
            kb_changed,
        }
    }

    /// Return tier capabilities (for Tauri command `get_tier_capabilities`).
    pub async fn get_tier_capabilities(&self) -> TierCapabilities {
        let policy = self.last_policy.read().await.clone();

        match policy {
            // No policy has been received yet (first boot with no cache, or server
            // unreachable before any heartbeat).  Fail closed: restrict all
            // sensitive capabilities until a policy is confirmed.  This prevents
            // an attacker from exploiting the pre-heartbeat window.
            None => TierCapabilities {
                tier: None,
                voice_available: false,
                multi_channel: false,
                max_channels: Some(1),
                autonomous_mode: false,
                k2k_federation: false,
                computer_use: false,
                scheduled_tasks: false,
                browser_tool: false,
                execute_tool: false,
                filesystem_write: false,
                credits_remaining: None,
                // No policy = DLP must be enforced (fail-closed)
                dlp_required: true,
            },
            Some(p) => {
                let tr = p.tool_restrictions.as_ref();
                TierCapabilities {
                    tier: p.tier.clone(),
                    voice_available: p.voice_enabled.unwrap_or(true),
                    multi_channel: p.max_channels.map(|m| m > 1).unwrap_or(true),
                    max_channels: p.max_channels,
                    autonomous_mode: p.autonomy_level.as_deref() != Some("blocked"),
                    k2k_federation: p.k2k_federation_enabled.unwrap_or(true),
                    computer_use: p.computer_use_enabled.unwrap_or(true),
                    scheduled_tasks: p.scheduled_tasks_enabled.unwrap_or(true),
                    browser_tool: tr.and_then(|t| t.browser_allowed).unwrap_or(true),
                    execute_tool: tr.and_then(|t| t.execute_allowed).unwrap_or(true),
                    filesystem_write: tr.and_then(|t| t.filesystem_write_allowed).unwrap_or(true),
                    credits_remaining: p.credits_remaining,
                    // dlp_enabled absent or true → DLP required; false → DLP explicitly not required
                    dlp_required: p.dlp_enabled.unwrap_or(true),
                }
            }
        }
    }

    /// Trigger an immediate policy refresh (for Tauri command `force_policy_refresh`).
    pub async fn force_refresh(&self) -> Result<()> {
        let client = self.make_client().await?;
        let instance_id = {
            let cfg = self.config.read().await;
            cfg.managed_policy.instance_id.clone()
        };
        if let Some(id) = instance_id {
            self.fetch_and_apply(&client, &id).await;
            Ok(())
        } else {
            Err(anyhow::anyhow!("No instance_id registered yet"))
        }
    }

    // -----------------------------------------------------------------------
    // Internal
    // -----------------------------------------------------------------------

    async fn run(&self) -> Result<()> {
        // --- Startup policy cache ---
        // Apply any policy that was persisted to the config during a previous
        // session.  This closes the unrestricted window that would otherwise
        // exist between app launch and the first successful heartbeat.
        {
            let cached_value = self.config.read().await.managed_policy.cached_policy.clone();
            if let Some(value) = cached_value {
                match serde_json::from_value::<ManagedPolicy>(value) {
                    Ok(policy) => {
                        info!(
                            "[MANAGED_POLICY] Applying startup-cached policy version={}",
                            policy.version_string()
                        );
                        *self.current_policy_version.write().await = Some(policy.version_string());
                        *self.last_policy.write().await = Some(policy.clone());
                        self.apply_policy(&policy).await;
                    }
                    Err(e) => {
                        warn!("[MANAGED_POLICY] Failed to deserialize cached policy: {}", e);
                    }
                }
            }
        }

        let client = self.make_client().await?;
        let instance_id = self.register_and_apply_policy(&client).await?;

        // Fetch and apply the latest policy (updates or confirms the startup-cached one).
        self.fetch_and_apply(&client, &instance_id).await;

        // Heartbeat loop.
        let interval_secs = self
            .config
            .read()
            .await
            .managed_policy
            .heartbeat_interval_secs;
        let mut interval = tokio::time::interval(Duration::from_secs(interval_secs.max(30)));
        interval.tick().await; // consume the immediate first tick

        loop {
            interval.tick().await;
            if !self.heartbeat_once(&client, &instance_id).await {
                // Server issued Disconnect/Suspend — stop the loop.
                break;
            }
        }
        Ok(())
    }

    /// Build a `KnManagementClient` from the current config.
    async fn make_client(&self) -> Result<KnManagementClient> {
        let cfg = self.config.read().await;
        let mp = &cfg.managed_policy;
        let token = mp
            .service_token
            .clone()
            .ok_or_else(|| anyhow::anyhow!("managed_policy.service_token is not set"))?;
        Ok(KnManagementClient::new(&mp.kn_server_url, token))
    }

    /// Ensure `instance_id` exists in config (generate UUID if absent),
    /// register with KN, apply the returned policy, and return the instance_id.
    async fn register_and_apply_policy(&self, client: &KnManagementClient) -> Result<String> {
        // Ensure we have a stable instance_id.
        let instance_id = {
            let cfg = self.config.read().await;
            cfg.managed_policy.instance_id.clone()
        };
        let instance_id = if let Some(id) = instance_id {
            id
        } else {
            let new_id = uuid::Uuid::new_v4().to_string();
            let mut cfg = self.config.write().await;
            cfg.managed_policy.instance_id = Some(new_id.clone());
            if let Err(e) = cfg.save() {
                warn!("[MANAGED_POLICY] Failed to persist new instance_id: {}", e);
            }
            new_id
        };

        let registration = InstanceRegistration {
            instance_id: instance_id.clone(),
            device_name: None, // optional; omit to avoid pulling in extra deps
            platform: Some(platform_string().to_string()),
            nexibot_version: Some(env!("CARGO_PKG_VERSION").to_string()),
        };

        match client.register_instance(&registration).await {
            Ok(resp) => {
                info!(
                    "[MANAGED_POLICY] Registered — instance_id={}, status={}, tier={}",
                    resp.instance_id,
                    resp.status,
                    resp.policy.tier.as_deref().unwrap_or("unknown"),
                );
                // Persist the server-assigned instance_id if it differs from ours.
                if instance_id != resp.instance_id {
                    let mut cfg = self.config.write().await;
                    cfg.managed_policy.instance_id = Some(resp.instance_id.clone());
                    if let Err(e) = cfg.save() {
                        warn!("[MANAGED_POLICY] Failed to persist instance_id: {}", e);
                    }
                }
                Ok(resp.instance_id)
            }
            Err(e) => {
                warn!(
                    "[MANAGED_POLICY] Registration failed ({}), continuing with cached instance_id={}",
                    e, instance_id
                );
                Ok(instance_id)
            }
        }
    }

    /// Fetch the policy from KN and apply floors/ceilings to the live config.
    async fn fetch_and_apply(&self, client: &KnManagementClient, instance_id: &str) {
        match client.fetch_managed_policy(instance_id).await {
            Ok(policy) => {
                let version = policy.version_string();
                info!("[MANAGED_POLICY] Fetched policy version={}", version);
                *self.current_policy_version.write().await = Some(version);
                *self.last_policy.write().await = Some(policy.clone());
                self.apply_policy(&policy).await;
                // Persist the policy so it can be re-applied on next startup
                // before the first successful heartbeat (closes unrestricted window).
                {
                    let mut cfg = self.config.write().await;
                    cfg.managed_policy.cached_policy = serde_json::to_value(&policy).ok();
                    if let Err(e) = cfg.save() {
                        warn!("[MANAGED_POLICY] Failed to persist cached_policy: {}", e);
                    }
                }
            }
            Err(e) => warn!("[MANAGED_POLICY] Failed to fetch policy: {}", e),
        }
    }

    /// Apply a policy that arrived inline in a heartbeat response.
    async fn apply_inline_policy(&self, policy: ManagedPolicy) {
        let version = policy.version_string();
        info!("[MANAGED_POLICY] Applying inline policy version={}", version);
        *self.current_policy_version.write().await = Some(version);
        *self.last_policy.write().await = Some(policy.clone());
        self.apply_policy(&policy).await;
        // Persist so the next startup re-applies this policy immediately.
        {
            let mut cfg = self.config.write().await;
            cfg.managed_policy.cached_policy = serde_json::to_value(&policy).ok();
            if let Err(e) = cfg.save() {
                warn!("[MANAGED_POLICY] Failed to persist cached_policy (inline): {}", e);
            }
        }
    }

    /// Send a heartbeat; apply the embedded policy if changed.
    /// Returns `false` if the server issued a Disconnect/Suspend — caller should stop.
    async fn heartbeat_once(&self, client: &KnManagementClient, instance_id: &str) -> bool {
        let current_version_i64 = self
            .current_policy_version
            .read()
            .await
            .as_deref()
            .and_then(|v| v.parse::<i64>().ok());
        let current_kb_version = *self.kb_version.read().await;

        let req = HeartbeatRequest {
            nexibot_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            current_policy_version: current_version_i64,
            compliance_hash: None,
            current_kb_version: Some(current_kb_version),
        };

        match client.send_heartbeat(instance_id, &req).await {
            Ok(resp) => {
                *self.last_heartbeat.write().await = Some(Instant::now());

                if resp.policy_changed {
                    if let Some(policy) = resp.policy {
                        self.apply_inline_policy(policy).await;
                    } else {
                        info!("[MANAGED_POLICY] Policy changed on server — re-fetching");
                        self.fetch_and_apply(client, instance_id).await;
                    }
                }

                // KB change detection.  When the server reports a version that
                // differs from what we sent, signal that the personal KB cache
                // is stale.  We update our tracked kb_version immediately so
                // that the next heartbeat reports the new version — confirming
                // to the server that we received and acknowledged the signal.
                *self.kb_changed.write().await = resp.kb_changed;
                if resp.kb_changed {
                    info!(
                        "[MANAGED_POLICY] Personal KB changed — server kb_version={} \
                        (was {}). KB cache should be refreshed.",
                        resp.kb_version, current_kb_version
                    );
                    *self.kb_version.write().await = resp.kb_version;
                }

                // Handle server directives.
                if let Some(cmd) = resp.pending_command.as_deref() {
                    match cmd {
                        "FetchPolicy" => {
                            info!("[MANAGED_POLICY] Server requested immediate policy refresh");
                            if let Ok(policy) = client.fetch_managed_policy(instance_id).await {
                                let version = policy.version_string();
                                self.apply_policy(&policy).await;
                                *self.current_policy_version.write().await = Some(version);
                            }
                        }
                        "Disconnect" | "Suspend" => {
                            warn!(
                                "[MANAGED_POLICY] Server issued '{}' — \
                                NexiBot will stop accepting new messages. \
                                Check your Knowledge Nexus subscription.",
                                cmd
                            );
                            let mut cfg = self.config.write().await;
                            cfg.managed_policy.enabled = false;
                            return false; // signal caller to stop the loop
                        }
                        other => {
                            warn!("[MANAGED_POLICY] Unknown pending_command '{}' — ignoring", other);
                        }
                    }
                }
                true
            }
            Err(e) => {
                warn!("[MANAGED_POLICY] Heartbeat failed: {}", e);
                true // transient failure — keep looping
            }
        }
    }

    /// Apply managed policy floors/ceilings to the live NexiBotConfig.
    ///
    /// Floors raise restrictions; ceilings cap capabilities.
    /// Neither direction overrides a locally-configured value that is already
    /// more restrictive.
    async fn apply_policy(&self, policy: &ManagedPolicy) {
        let mut cfg = self.config.write().await;

        // --- 1. Guardrails floor ---
        // SecurityLevel ordering: Maximum=0, Standard=1, Relaxed=2, Disabled=3.
        // Higher numeric value = less secure.
        // Server levels: "maximum"/"strict" > "standard" > "relaxed".
        // "strict" has no direct local equivalent — mapped to Maximum (conservative).
        if let Some(floor_str) = &policy.guardrails_floor {
            if let Some(floor_level) = parse_server_guardrails_floor(floor_str) {
                let current = cfg.guardrails.security_level;
                if (current as u8) > (floor_level as u8) {
                    info!(
                        "[MANAGED_POLICY] Raising guardrails from {:?} to {:?} (KN floor: {})",
                        current, floor_level, floor_str
                    );
                    cfg.guardrails.security_level = floor_level;
                }
            }
        }

        // --- 2. Allowed models (restrict to intersection) ---
        if let Some(allowed) = &policy.allowed_models {
            // ["*"] means all models are permitted — no restriction.
            if !allowed.is_empty() && !allowed.contains(&"*".to_string()) {
                let current = &cfg.claude.model;
                if !allowed.contains(current) {
                    if let Some(first) = allowed.first() {
                        warn!(
                            "[MANAGED_POLICY] Model '{}' not in KN allowed list — switching to '{}'",
                            current, first
                        );
                        cfg.claude.model = first.clone();
                    }
                }
            }
        }

        // --- 3. Feature flags (advisory) ---
        for (key, value) in &policy.feature_flags {
            info!("[MANAGED_POLICY] Feature flag: {} = {}", key, value);
        }

        // --- 4. Voice enable/disable ---
        if policy.voice_enabled == Some(false) {
            if cfg.wakeword.enabled {
                info!("[MANAGED_POLICY] Disabling voice (policy floor)");
                cfg.wakeword.enabled = false;
            }
        }

        // --- 5. Channel restrictions ---
        if let Some(max) = policy.max_channels {
            let active = count_enabled_channels(&cfg);
            if active > max {
                warn!(
                    "[MANAGED_POLICY] Active channels ({}) exceeds policy max ({}) — \
                    disabling excess channels",
                    active, max
                );
                disable_excess_channels(&mut cfg, max);
            }
        }

        // --- 5b. Allowed channel types ---
        // If the policy restricts which channel *types* are permitted, disable any
        // enabled channel whose type is not in the allowlist.
        if let Some(allowed_types) = &policy.allowed_channel_types {
            if !allowed_types.is_empty() {
                macro_rules! disable_if_not_allowed {
                    ($field:expr, $name:expr) => {
                        if $field && !allowed_types.iter().any(|t| t.eq_ignore_ascii_case($name)) {
                            info!(
                                "[MANAGED_POLICY] Disabling channel '{}' — not in allowed_channel_types",
                                $name
                            );
                            $field = false;
                        }
                    };
                }
                disable_if_not_allowed!(cfg.telegram.enabled,    "telegram");
                disable_if_not_allowed!(cfg.whatsapp.enabled,    "whatsapp");
                disable_if_not_allowed!(cfg.discord.enabled,     "discord");
                disable_if_not_allowed!(cfg.slack.enabled,       "slack");
                disable_if_not_allowed!(cfg.signal.enabled,      "signal");
                disable_if_not_allowed!(cfg.teams.enabled,       "teams");
                disable_if_not_allowed!(cfg.matrix.enabled,      "matrix");
                disable_if_not_allowed!(cfg.email.enabled,       "email");
                disable_if_not_allowed!(cfg.gateway.enabled,     "gateway");
                disable_if_not_allowed!(cfg.bluebubbles.enabled, "bluebubbles");
                disable_if_not_allowed!(cfg.google_chat.enabled, "google_chat");
                disable_if_not_allowed!(cfg.mattermost.enabled,  "mattermost");
                disable_if_not_allowed!(cfg.messenger.enabled,   "messenger");
                disable_if_not_allowed!(cfg.instagram.enabled,   "instagram");
                disable_if_not_allowed!(cfg.line.enabled,        "line");
                disable_if_not_allowed!(cfg.twilio.enabled,      "twilio");
                disable_if_not_allowed!(cfg.mastodon.enabled,    "mastodon");
                disable_if_not_allowed!(cfg.rocketchat.enabled,  "rocketchat");
                disable_if_not_allowed!(cfg.webchat.enabled,     "webchat");
            }
        }

        // --- 6. Autonomy ceiling ---
        if let Some(autonomy) = &policy.autonomy_level {
            apply_autonomy_ceiling(&mut cfg, autonomy);
        }

        // --- 7. Tool restrictions ---
        if let Some(restrictions) = &policy.tool_restrictions {
            if restrictions.execute_allowed == Some(false) && cfg.execute.enabled {
                info!("[MANAGED_POLICY] Disabling execute tool (policy restriction)");
                cfg.execute.enabled = false;
            }
            if restrictions.filesystem_write_allowed == Some(false) {
                // When write is blocked, ensure write mode is off (if the field exists).
                // The filesystem config has an `enabled` flag; we can't set per-op
                // permissions directly here, so we log for advisory enforcement.
                info!("[MANAGED_POLICY] filesystem_write_allowed=false (advisory — UI should reflect this)");
            }
            if restrictions.filesystem_delete_allowed == Some(false) {
                info!("[MANAGED_POLICY] filesystem_delete_allowed=false (advisory)");
            }
            if restrictions.browser_allowed == Some(false) && cfg.browser.enabled {
                info!("[MANAGED_POLICY] Disabling browser tool (policy restriction)");
                cfg.browser.enabled = false;
            }
            if restrictions.fetch_allowed == Some(false) && cfg.fetch.enabled {
                info!("[MANAGED_POLICY] Disabling fetch tool (policy restriction)");
                cfg.fetch.enabled = false;
            }
        }

        // --- 7b. API access (fetch tool) ---
        // `api_access_enabled = false` prevents the fetch tool from making outbound
        // HTTP requests.  This is distinct from `tool_restrictions.fetch_allowed`
        // which controls fine-grained tool permission; this flag is the tier-level
        // switch used by subscription enforcement.
        if policy.api_access_enabled == Some(false) && cfg.fetch.enabled {
            info!("[MANAGED_POLICY] Disabling fetch tool (api_access_enabled=false policy restriction)");
            cfg.fetch.enabled = false;
        }

        // --- 8. K2K federation ---
        if policy.k2k_federation_enabled == Some(false) && cfg.k2k.enabled {
            info!("[MANAGED_POLICY] Disabling K2K federation (policy restriction)");
            cfg.k2k.enabled = false;
        }

        // --- 9. Computer Use ---
        if policy.computer_use_enabled == Some(false) && cfg.computer_use.enabled {
            info!("[MANAGED_POLICY] Disabling Computer Use (policy restriction)");
            cfg.computer_use.enabled = false;
        }

        // --- 10. Scheduled tasks ---
        if policy.scheduled_tasks_enabled == Some(false) {
            if cfg.scheduled_tasks.enabled {
                info!("[MANAGED_POLICY] Disabling scheduled tasks (policy restriction)");
                cfg.scheduled_tasks.enabled = false;
            }
        }

        // --- 10b. DLP enforcement ---
        // `dlp_enabled = false` means the server is REVOKING DLP exemption —
        // DLP scanning must be enforced.  When the field is absent we treat it
        // as `true` (DLP always required by default).  This is intentionally
        // fail-closed: if the server can't communicate intent, keep scanning.
        // Note: The guardrails layer owns the actual outbound content scanner;
        // raising to Standard or higher automatically enables DLP patterns.
        // If DLP was explicitly allowed (`dlp_enabled = Some(true)`) we have
        // nothing extra to do.  If it's `Some(false)` the subscription does
        // NOT exempt DLP — ensure guardrails are at least Standard.
        if policy.dlp_enabled == Some(false) {
            use crate::guardrails::SecurityLevel;
            let current = cfg.guardrails.security_level;
            if (current as u8) > (SecurityLevel::Standard as u8) {
                warn!(
                    "[MANAGED_POLICY] dlp_enabled=false — raising guardrails to Standard \
                    to enforce DLP scanning"
                );
                cfg.guardrails.security_level = SecurityLevel::Standard;
            }
        }

        // --- 11. Process embedded PolicyCommands ---
        for cmd in &policy.commands {
            match cmd {
                PolicyCommand::Disconnect { reason } => {
                    warn!(
                        "[MANAGED_POLICY] Policy contains Disconnect command (reason: {}) — \
                        disabling managed policy",
                        reason
                    );
                    cfg.managed_policy.enabled = false;
                }
                PolicyCommand::ShowNotification { message, level } => {
                    // We don't have access to the AppHandle here; log and rely on
                    // the Tauri command layer to relay this to the frontend.
                    info!(
                        "[MANAGED_POLICY] Notification [{}]: {}",
                        level, message
                    );
                }
                PolicyCommand::FetchPolicy => {
                    // Will be handled in the heartbeat/fetch loop after apply_policy returns.
                    info!("[MANAGED_POLICY] Policy contains FetchPolicy directive");
                }
            }
        }

        // --- 12. Policy expiry check (offline grace period) ---
        if let Some(expires_at) = policy.expires_at {
            let now_unix = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            if now_unix > expires_at {
                warn!(
                    "[MANAGED_POLICY] Policy expired (expires_at={}, now={}) — \
                    applying safe defaults",
                    expires_at, now_unix
                );
                apply_safe_defaults(&mut cfg);
            }
        }

        // Persist the updated config (best-effort).
        if let Err(e) = cfg.save() {
            warn!("[MANAGED_POLICY] Failed to persist policy-updated config: {}", e);
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Map the server's guardrails floor string to a local SecurityLevel.
///
/// Server values (least → most restrictive): "relaxed", "standard", "strict", "maximum".
/// Local SecurityLevel ordinal (most → least secure): Maximum=0, Standard=1, Relaxed=2, Disabled=3.
///
/// "strict" has no direct local equivalent — mapped to Maximum (conservative).
fn parse_server_guardrails_floor(s: &str) -> Option<SecurityLevel> {
    match s.to_lowercase().as_str() {
        "maximum" | "strict" => Some(SecurityLevel::Maximum),
        "standard"           => Some(SecurityLevel::Standard),
        "relaxed"            => Some(SecurityLevel::Relaxed),
        _ => {
            warn!("[MANAGED_POLICY] Unknown guardrails floor '{}' — ignoring", s);
            None
        }
    }
}

/// Count the number of currently enabled external channels.
fn count_enabled_channels(cfg: &NexiBotConfig) -> usize {
    [
        cfg.telegram.enabled, cfg.whatsapp.enabled, cfg.discord.enabled,
        cfg.slack.enabled, cfg.signal.enabled, cfg.teams.enabled, cfg.matrix.enabled,
        cfg.email.enabled, cfg.gateway.enabled, cfg.bluebubbles.enabled,
        cfg.google_chat.enabled, cfg.mattermost.enabled, cfg.messenger.enabled,
        cfg.instagram.enabled, cfg.line.enabled, cfg.twilio.enabled,
        cfg.mastodon.enabled, cfg.rocketchat.enabled, cfg.webchat.enabled,
    ]
    .iter()
    .filter(|&&e| e)
    .count()
}

/// Return the OS platform string expected by the KN API.
fn platform_string() -> &'static str {
    if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    }
}

/// Disable channels beyond `max`, starting with the least common ones
/// (alphabetical tail) to be deterministic.
fn disable_excess_channels(cfg: &mut NexiBotConfig, max: usize) {
    // Ordered list of (flag_ref, name) from least to most "core".
    // We disable in reverse priority order until within the limit.
    macro_rules! disable_if {
        ($field:expr, $name:expr) => {
            if count_enabled_channels(cfg) > max && $field {
                info!("[MANAGED_POLICY] Disabling channel '{}' to stay within max_channels={}", $name, max);
                $field = false;
            }
        };
    }
    disable_if!(cfg.webchat.enabled,      "webchat");
    disable_if!(cfg.rocketchat.enabled,   "rocketchat");
    disable_if!(cfg.mastodon.enabled,     "mastodon");
    disable_if!(cfg.twilio.enabled,       "twilio");
    disable_if!(cfg.line.enabled,         "line");
    disable_if!(cfg.instagram.enabled,    "instagram");
    disable_if!(cfg.messenger.enabled,    "messenger");
    disable_if!(cfg.mattermost.enabled,   "mattermost");
    disable_if!(cfg.google_chat.enabled,  "google_chat");
    disable_if!(cfg.bluebubbles.enabled,  "bluebubbles");
    disable_if!(cfg.email.enabled,        "email");
    disable_if!(cfg.matrix.enabled,       "matrix");
    disable_if!(cfg.teams.enabled,        "teams");
    disable_if!(cfg.signal.enabled,       "signal");
    disable_if!(cfg.slack.enabled,        "slack");
    disable_if!(cfg.discord.enabled,      "discord");
    disable_if!(cfg.whatsapp.enabled,     "whatsapp");
    disable_if!(cfg.telegram.enabled,     "telegram");
    disable_if!(cfg.gateway.enabled,      "gateway");
}

/// Apply the autonomy ceiling from the policy.
/// "blocked" > "ask_user" > "autonomous" — if local is higher than ceiling, lower it.
fn apply_autonomy_ceiling(cfg: &mut NexiBotConfig, ceiling: &str) {
    if !cfg.autonomous_mode.enabled {
        return; // nothing to cap
    }
    match ceiling {
        "blocked" => {
            info!("[MANAGED_POLICY] Autonomy ceiling=blocked — disabling autonomous mode");
            cfg.autonomous_mode.enabled = false;
        }
        "ask_user" => {
            // Keep autonomous mode enabled but ensure all capability levels are
            // at most AskUser, not Autonomous.
            use crate::config::AutonomyLevel;
            macro_rules! cap_level {
                ($field:expr) => {
                    if $field == AutonomyLevel::Autonomous {
                        $field = AutonomyLevel::AskUser;
                    }
                };
            }
            cap_level!(cfg.autonomous_mode.filesystem.read);
            cap_level!(cfg.autonomous_mode.filesystem.write);
            cap_level!(cfg.autonomous_mode.fetch.get_requests);
            cap_level!(cfg.autonomous_mode.fetch.post_requests);
            cap_level!(cfg.autonomous_mode.execute.run_command);
            cap_level!(cfg.autonomous_mode.execute.run_python);
            cap_level!(cfg.autonomous_mode.execute.run_node);
            cap_level!(cfg.autonomous_mode.browser.navigate);
            cap_level!(cfg.autonomous_mode.browser.interact);
            info!("[MANAGED_POLICY] Autonomy ceiling=ask_user applied");
        }
        "autonomous" => {
            // No cap needed — autonomous is the maximum level.
        }
        other => {
            warn!("[MANAGED_POLICY] Unknown autonomy_level '{}' in policy — ignoring", other);
        }
    }
}

/// Revert config to conservative defaults when the cached policy has expired.
fn apply_safe_defaults(cfg: &mut NexiBotConfig) {
    use crate::guardrails::SecurityLevel;
    // Raise guardrails to at least Standard.
    if (cfg.guardrails.security_level as u8) > (SecurityLevel::Standard as u8) {
        cfg.guardrails.security_level = SecurityLevel::Standard;
    }
    // Disable autonomous mode.
    cfg.autonomous_mode.enabled = false;
    // Disable execute and browser tools.
    cfg.execute.enabled = false;
    cfg.browser.enabled = false;
    info!("[MANAGED_POLICY] Safe defaults applied due to expired policy");
}
