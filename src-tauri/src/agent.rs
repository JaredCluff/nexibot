//! Multi-Agent Architecture for NexiBot.
//!
//! Supports config-defined agents with per-agent SOUL, model,
//! channel bindings, full conversation isolation, and inter-agent messaging.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

use crate::claude::ClaudeClient;
use crate::config::{AgentConfig, NexiBotConfig};
use crate::session_overrides::SessionOverrides;
use crate::soul::Soul;

/// A running agent instance with its own Claude client and personality.
#[allow(dead_code)]
pub struct AgentInstance {
    pub config: AgentConfig,
    pub claude_client: ClaudeClient,
    pub soul: Option<Soul>,
    pub session_overrides: SessionOverrides,
}

/// Serializable agent info for the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub id: String,
    pub name: String,
    pub avatar: Option<String>,
    pub model: Option<String>,
    pub is_default: bool,
    pub channel_bindings: Vec<crate::config::ChannelBinding>,
}

/// Manages multiple agent instances and routes messages to the correct agent.
pub struct AgentManager {
    agents: HashMap<String, AgentInstance>,
    default_agent_id: Option<String>,
    /// Which agent the desktop GUI currently talks to.
    pub active_gui_agent_id: String,
}

impl AgentManager {
    /// Create an AgentManager from config. If no agents are configured,
    /// creates an implicit "main" agent for backward compatibility.
    pub fn new(config: &NexiBotConfig, global_config: Arc<RwLock<NexiBotConfig>>) -> Self {
        let mut agents = HashMap::new();
        let mut default_agent_id = None;

        if config.agents.is_empty() {
            // Backward compatible: create implicit "main" agent
            let client = ClaudeClient::new(global_config.clone());
            let soul = Soul::load().ok();

            agents.insert(
                "main".to_string(),
                AgentInstance {
                    config: AgentConfig {
                        id: "main".to_string(),
                        name: "NexiBot".to_string(),
                        avatar: None,
                        model: None,
                        primary_model: None,
                        backup_model: None,
                        provider: None,
                        soul_path: None,
                        system_prompt: None,
                        is_default: true,
                        channel_bindings: Vec::new(),
                        capabilities: Vec::new(),
                    },
                    claude_client: client,
                    soul,
                    session_overrides: SessionOverrides::default(),
                },
            );
            default_agent_id = Some("main".to_string());

            info!("[AGENT] Created implicit 'main' agent (no agents configured)");
        } else {
            for agent_config in &config.agents {
                let client = ClaudeClient::new(global_config.clone());

                // Load per-agent soul if configured
                let soul = if let Some(ref soul_path) = agent_config.soul_path {
                    Soul::load_from_path(soul_path).ok()
                } else {
                    Soul::load().ok()
                };

                if agent_config.is_default {
                    default_agent_id = Some(agent_config.id.clone());
                }

                info!(
                    "[AGENT] Created agent '{}' ({})",
                    agent_config.id, agent_config.name
                );
                agents.insert(
                    agent_config.id.clone(),
                    AgentInstance {
                        config: agent_config.clone(),
                        claude_client: client,
                        soul,
                        session_overrides: SessionOverrides::default(),
                    },
                );
            }

            // If no agent is marked as default, use the first one
            if default_agent_id.is_none() {
                if let Some(first_id) = config.agents.first().map(|a| a.id.clone()) {
                    default_agent_id = Some(first_id);
                }
            }
        }

        let active_gui_agent_id = default_agent_id
            .clone()
            .unwrap_or_else(|| "main".to_string());

        Self {
            agents,
            default_agent_id,
            active_gui_agent_id,
        }
    }

    /// Resolve which agent should handle a message from a given channel and peer.
    /// Priority: exact peer match > channel-only match > default agent.
    pub fn resolve_agent(&self, channel: &str, peer_id: &str) -> &str {
        // 1. Exact peer match
        for (id, instance) in &self.agents {
            for binding in &instance.config.channel_bindings {
                if binding.channel == channel {
                    if let Some(ref bound_peer) = binding.peer_id {
                        if bound_peer == peer_id {
                            return id;
                        }
                    }
                }
            }
        }

        // 2. Channel-only match (peer_id is None)
        for (id, instance) in &self.agents {
            for binding in &instance.config.channel_bindings {
                if binding.channel == channel && binding.peer_id.is_none() {
                    return id;
                }
            }
        }

        // 3. Default agent
        self.default_agent_id.as_deref().unwrap_or("main")
    }

    /// Get an agent by ID.
    pub fn get_agent(&self, id: &str) -> Option<&AgentInstance> {
        self.agents.get(id)
    }

    /// Get a mutable reference to an agent by ID.
    #[allow(dead_code)]
    pub fn get_agent_mut(&mut self, id: &str) -> Option<&mut AgentInstance> {
        self.agents.get_mut(id)
    }

    /// List all agents as serializable info.
    pub fn list_agents(&self) -> Vec<AgentInfo> {
        self.agents
            .values()
            .map(|a| AgentInfo {
                id: a.config.id.clone(),
                name: a.config.name.clone(),
                avatar: a.config.avatar.clone(),
                model: a.config.model.clone(),
                is_default: a.config.is_default,
                channel_bindings: a.config.channel_bindings.clone(),
            })
            .collect()
    }

    /// Get the default agent.
    pub fn default_agent(&self) -> Option<&AgentInstance> {
        self.default_agent_id
            .as_ref()
            .and_then(|id| self.agents.get(id))
    }

    /// Get the default agent ID.
    #[allow(dead_code)]
    pub fn default_agent_id(&self) -> Option<&str> {
        self.default_agent_id.as_deref()
    }

    /// Rebuild all agent instances from the latest config.
    ///
    /// Preserves `active_gui_agent_id` if the agent still exists in the new
    /// config; otherwise resets it to the new default.
    pub fn reload_agents(&mut self, config: &NexiBotConfig, global_config: Arc<RwLock<NexiBotConfig>>) {
        let previous_gui_id = self.active_gui_agent_id.clone();

        let rebuilt = AgentManager::new(config, global_config);
        self.agents = rebuilt.agents;
        self.default_agent_id = rebuilt.default_agent_id;

        // Keep the GUI-active agent if it still exists, else fall back to default
        if self.agents.contains_key(&previous_gui_id) {
            self.active_gui_agent_id = previous_gui_id;
        } else {
            self.active_gui_agent_id = self.default_agent_id
                .clone()
                .unwrap_or_else(|| "main".to_string());
            info!(
                "[AGENT] Previous GUI agent '{}' no longer exists, switched to '{}'",
                previous_gui_id, self.active_gui_agent_id
            );
        }

        info!(
            "[AGENT] Reloaded {} agent(s) (active GUI: {})",
            self.agents.len(),
            self.active_gui_agent_id
        );
    }
}
