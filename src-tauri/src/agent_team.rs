//! Agent team coordination with K2K-native task delegation.
//!
//! Supports local agent capability discovery and task delegation,
//! with transparent fallback to K2K federation for remote agents.
#![allow(dead_code)]

use std::collections::{HashMap, HashSet};

use chrono::Utc;
use k2k::models::{
    AgentCapability, CapabilitiesResponse, CapabilityCategory, TaskRequest, TaskResult, TaskStatus,
    TaskStatusResponse,
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::agent::AgentManager;
use crate::config::AgentCapabilityConfig;
use crate::k2k_client::K2KIntegration;

// ---------------------------------------------------------------------------
// Stop words for TF-IDF scoring
// ---------------------------------------------------------------------------

const STOP_WORDS: &[&str] = &[
    "a", "an", "the", "is", "are", "was", "were", "be", "been", "being", "have", "has", "had",
    "do", "does", "did", "will", "would", "could", "should", "may", "might", "shall", "can",
    "need", "to", "of", "in", "for", "on", "with", "at", "by", "from", "as", "into", "through",
    "during", "before", "after", "above", "below", "between", "out", "off", "over", "under",
    "again", "further", "then", "once", "here", "there", "when", "where", "why", "how", "all",
    "both", "each", "few", "more", "most", "other", "some", "such", "no", "nor", "not", "only",
    "own", "same", "so", "than", "too", "very", "just", "about", "and", "but", "or", "if", "while",
    "that", "this", "it", "i", "me", "my", "we", "our", "you", "your", "he", "she", "they", "them",
    "what", "which", "who", "whom",
];

/// Score a query against a single capability using TF-IDF-like scoring.
fn score_capability(query_words: &[String], cap: &AgentCapability) -> f64 {
    let cap_name_lower = cap.name.to_lowercase();
    let cap_desc_lower = cap.description.to_lowercase();

    let mut score: f64 = 0.0;

    // Exact phrase match in name → strong signal
    let query_joined = query_words.join(" ");
    if cap_name_lower.contains(&query_joined) || query_joined.contains(&cap_name_lower) {
        score += 2.0;
    }

    // Exact phrase match in description → moderate signal
    if cap_desc_lower.contains(&query_joined) {
        score += 1.0;
    }

    // Per-word scoring with specificity weighting
    let cap_name_words: Vec<String> = tokenize(&cap_name_lower);
    let cap_desc_words: Vec<String> = tokenize(&cap_desc_lower);

    for word in query_words {
        // Name match is weighted higher than description match
        if cap_name_words.contains(word) {
            // Shorter, more specific capability names score higher
            let specificity = 1.0 / (cap_name_words.len() as f64).max(1.0);
            score += 0.5 + specificity;
        }
        if cap_desc_words.contains(word) {
            let specificity = 1.0 / (cap_desc_words.len() as f64).max(1.0);
            score += 0.2 + specificity * 0.5;
        }
    }

    score
}

/// Tokenize a string into lowercase words, removing stop words.
fn tokenize(text: &str) -> Vec<String> {
    let stop: HashSet<&str> = STOP_WORDS.iter().copied().collect();
    text.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .filter(|w| !w.is_empty() && w.len() > 1)
        .map(|w| w.to_lowercase())
        .filter(|w| !stop.contains(w.as_str()))
        .collect()
}

/// Scored agent match result.
#[derive(Debug, Clone)]
pub struct ScoredAgent {
    pub agent_id: String,
    pub score: f64,
    pub matched_capability: String,
}

/// Manages agent capabilities, task delegation, and cross-agent coordination.
pub struct AgentOrchestrator {
    /// Local agent capabilities registry: agent_id -> capabilities.
    capability_registry: RwLock<HashMap<String, Vec<AgentCapability>>>,
    /// Active delegated tasks: task_id -> status.
    /// Wrapped in Arc so it can be shared with background handler tasks.
    active_tasks: std::sync::Arc<RwLock<HashMap<String, TaskStatusResponse>>>,
    /// Counter for generating local task IDs.
    task_counter: std::sync::atomic::AtomicU64,
}

/// Serializable capability info for the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCapabilities {
    pub agent_id: String,
    pub agent_name: String,
    pub capabilities: Vec<AgentCapability>,
}

impl AgentOrchestrator {
    /// Create a new orchestrator.
    pub fn new() -> Self {
        Self {
            capability_registry: RwLock::new(HashMap::new()),
            active_tasks: std::sync::Arc::new(RwLock::new(HashMap::new())),
            task_counter: std::sync::atomic::AtomicU64::new(1),
        }
    }

    /// Register capabilities for a local agent from its config.
    pub async fn register_agent_capabilities(
        &self,
        agent_id: &str,
        capabilities: &[AgentCapabilityConfig],
    ) {
        let caps: Vec<AgentCapability> = capabilities
            .iter()
            .map(|c| AgentCapability {
                id: format!("{}:{}", agent_id, c.name),
                name: c.name.clone(),
                category: match c.category.as_str() {
                    "knowledge" => CapabilityCategory::Knowledge,
                    "tool" => CapabilityCategory::Tool,
                    "compute" => CapabilityCategory::Compute,
                    _ => CapabilityCategory::Skill,
                },
                description: c.description.clone(),
                input_schema: None,
                version: "1.0".to_string(),
                min_protocol_version: None,
            })
            .collect();

        if !caps.is_empty() {
            info!(
                "[ORCHESTRATOR] Registered {} capabilities for agent '{}'",
                caps.len(),
                agent_id
            );
        }

        self.capability_registry
            .write()
            .await
            .insert(agent_id.to_string(), caps);
    }

    /// Initialize capability registry from all configured agents.
    pub async fn initialize_from_agents(&self, agent_manager: &AgentManager) {
        for agent_info in agent_manager.list_agents() {
            if let Some(agent) = agent_manager.get_agent(&agent_info.id) {
                self.register_agent_capabilities(&agent_info.id, &agent.config.capabilities)
                    .await;
            }
        }
    }

    /// Find the best local agent for a capability description using TF-IDF scoring.
    ///
    /// Scores all agents and returns the best match above the minimum threshold.
    /// If `category` is provided, agents in that category get a scoring bonus.
    pub async fn find_local_agent_for_capability(
        &self,
        category: Option<CapabilityCategory>,
        description: &str,
    ) -> Option<String> {
        self.find_best_agent(category, description)
            .await
            .map(|s| s.agent_id)
    }

    /// Find the best matching agent with full scoring details.
    ///
    /// Uses TF-IDF-like scoring: tokenizes the query, removes stop words,
    /// then scores each capability by word match weight * specificity.
    /// Returns None if no agent scores above the minimum threshold (0.1).
    pub async fn find_best_agent(
        &self,
        category: Option<CapabilityCategory>,
        description: &str,
    ) -> Option<ScoredAgent> {
        const MIN_SCORE_THRESHOLD: f64 = 0.1;

        let registry = self.capability_registry.read().await;
        let query_words = tokenize(description);

        if query_words.is_empty() {
            return None;
        }

        let mut best: Option<ScoredAgent> = None;

        for (agent_id, caps) in registry.iter() {
            for cap in caps {
                let mut score = score_capability(&query_words, cap);

                // Category bonus: matching category gets a boost
                if let Some(ref cat) = category {
                    if std::mem::discriminant(&cap.category) == std::mem::discriminant(cat) {
                        score *= 1.5;
                    }
                }

                if score >= MIN_SCORE_THRESHOLD {
                    if best.as_ref().map_or(true, |b| score > b.score) {
                        best = Some(ScoredAgent {
                            agent_id: agent_id.clone(),
                            score,
                            matched_capability: cap.name.clone(),
                        });
                    }
                }
            }
        }

        if let Some(ref matched) = best {
            info!(
                "[ORCHESTRATOR] Best agent for '{}': {} (score={:.2}, capability={})",
                description, matched.agent_id, matched.score, matched.matched_capability
            );
        }

        best
    }

    /// Score all agents against a description, returning them sorted by score.
    pub async fn rank_agents_for_capability(
        &self,
        category: Option<CapabilityCategory>,
        description: &str,
    ) -> Vec<ScoredAgent> {
        let registry = self.capability_registry.read().await;
        let query_words = tokenize(description);

        if query_words.is_empty() {
            return Vec::new();
        }

        let mut scored: Vec<ScoredAgent> = Vec::new();

        for (agent_id, caps) in registry.iter() {
            let mut best_cap_score: f64 = 0.0;
            let mut best_cap_name = String::new();

            for cap in caps {
                let mut score = score_capability(&query_words, cap);

                if let Some(ref cat) = category {
                    if std::mem::discriminant(&cap.category) == std::mem::discriminant(cat) {
                        score *= 1.5;
                    }
                }

                if score > best_cap_score {
                    best_cap_score = score;
                    best_cap_name = cap.name.clone();
                }
            }

            if best_cap_score > 0.0 {
                scored.push(ScoredAgent {
                    agent_id: agent_id.clone(),
                    score: best_cap_score,
                    matched_capability: best_cap_name,
                });
            }
        }

        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored
    }

    /// Delegate a task to a local agent and actually execute it.
    ///
    /// The `handler` closure receives the `TaskRequest` and returns a
    /// `TaskResult`.  It is run in a background tokio task so this function
    /// returns the task ID immediately; callers can poll via `poll_task`.
    ///
    /// Passing `None` for `handler` leaves the task in `Queued` state (useful
    /// in unit tests that only check routing logic).
    pub async fn delegate_local(
        &self,
        from_agent: &str,
        to_agent: &str,
        task: TaskRequest,
        handler: Option<
            std::sync::Arc<
                dyn Fn(TaskRequest) -> std::pin::Pin<
                        Box<dyn std::future::Future<Output = Result<TaskResult, String>> + Send>,
                    > + Send
                    + Sync,
            >,
        >,
    ) -> Result<String, String> {
        // Prevent self-delegation loops
        if to_agent == from_agent {
            return Err(format!("Agent '{}' cannot delegate to itself", from_agent));
        }

        // Verify the target agent exists in the capability registry
        {
            let registry = self.capability_registry.read().await;
            if !registry.contains_key(to_agent) {
                return Err(format!(
                    "Target agent '{}' is not registered in the local agent registry",
                    to_agent
                ));
            }
        }

        let task_id = format!(
            "local-{}",
            self.task_counter
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );

        let status = TaskStatusResponse {
            task_id: task_id.clone(),
            status: TaskStatus::Queued,
            result: None,
            error: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            progress: Some(0),
        };

        info!(
            "[ORCHESTRATOR] Local task {} created: {} -> {} (capability: {})",
            task_id, from_agent, to_agent, task.capability_id
        );

        self.active_tasks
            .write()
            .await
            .insert(task_id.clone(), status);

        // Spawn background execution if a handler was provided.
        if let Some(exec) = handler {
            let active_tasks: std::sync::Arc<RwLock<HashMap<String, TaskStatusResponse>>> =
                self.active_tasks.clone();
            let tid = task_id.clone();

            let _ = tokio::spawn(async move {
                // Mark running.
                {
                    let mut tasks = active_tasks.write().await;
                    if let Some(t) = tasks.get_mut(&tid) {
                        t.status = TaskStatus::Running;
                        t.progress = Some(10);
                        t.updated_at = Utc::now();
                    }
                }

                let result = exec(task).await;

                // Update final status.
                let mut tasks = active_tasks.write().await;
                if let Some(t) = tasks.get_mut(&tid) {
                    t.updated_at = Utc::now();
                    match result {
                        Ok(task_result) => {
                            t.status = TaskStatus::Completed;
                            t.result = Some(task_result);
                            t.progress = Some(100);
                        }
                        Err(e) => {
                            t.status = TaskStatus::Failed;
                            t.error = Some(e);
                            t.progress = None;
                        }
                    }
                }
            });
        }

        Ok(task_id)
    }

    /// Delegate a task, trying local first, then K2K federation.
    pub async fn delegate(
        &self,
        from_agent: &str,
        task: TaskRequest,
        k2k: &K2KIntegration,
    ) -> Result<String, String> {
        // 1. Try to find a local agent with matching capability
        let local_target = {
            let registry = self.capability_registry.read().await;
            let mut found = None;
            for (agent_id, caps) in registry.iter() {
                if agent_id == from_agent {
                    continue;
                } // Don't delegate to self
                for cap in caps {
                    if cap.id == task.capability_id || cap.name == task.capability_id {
                        found = Some(agent_id.clone());
                        break;
                    }
                }
                if found.is_some() {
                    break;
                }
            }
            found
        };

        if let Some(target_agent) = local_target {
            // Pass None for handler: the caller is responsible for wiring up
            // execution via delegate_local with a real handler when needed.
            // This routing-only path records the task and the caller drives it.
            return self
                .delegate_local(from_agent, &target_agent, task, None)
                .await;
        }

        // 2. Try K2K federation — but guard against routing to ourselves,
        //    which would create an infinite delegation cycle.
        if k2k.is_available().await {
            let client_guard = k2k.get_client().await;
            if let Some(ref client) = *client_guard {
                // Read the local node's own client_id so we can compare it
                // against the requesting_node_id in the task.  If the task
                // was already issued by this node we must not loop back to
                // ourselves via K2K.
                let config = k2k.get_config().await;
                let local_client_id = config.k2k.client_id.clone();
                let node_url = config.k2k.local_agent_url.clone();
                drop(config);

                // Self-routing guard: if the requesting node is us, fall
                // through to the "not found" error rather than creating a
                // circular K2K request.
                if task.requesting_node_id == local_client_id {
                    warn!(
                        "[ORCHESTRATOR] K2K delegation aborted: task requesting_node_id '{}' \
                         matches local client_id — routing to self would create a cycle",
                        local_client_id
                    );
                } else {
                    match client.submit_task(&node_url, &task).await {
                        Ok(response) => {
                            let task_id = response.task_id.clone();
                            let status = TaskStatusResponse {
                                task_id: task_id.clone(),
                                status: response.status,
                                result: None,
                                error: None,
                                created_at: Utc::now(),
                                updated_at: Utc::now(),
                                progress: Some(0),
                            };

                            self.active_tasks
                                .write()
                                .await
                                .insert(task_id.clone(), status);

                            info!(
                                "[ORCHESTRATOR] K2K task {} delegated via federation",
                                task_id
                            );
                            return Ok(task_id);
                        }
                        Err(e) => {
                            warn!("[ORCHESTRATOR] K2K delegation failed: {}", e);
                        }
                    }
                }
            }
        }

        Err(format!(
            "No agent found for capability '{}' (local or remote)",
            task.capability_id
        ))
    }

    /// Poll the status of a delegated task.
    pub async fn poll_task(&self, task_id: &str) -> Option<TaskStatusResponse> {
        self.active_tasks.read().await.get(task_id).cloned()
    }

    /// Update a local task's status (called by the executing agent).
    pub async fn update_task_status(
        &self,
        task_id: &str,
        status: TaskStatus,
        result: Option<TaskResult>,
        error: Option<String>,
        progress: Option<u8>,
    ) {
        let mut tasks = self.active_tasks.write().await;
        if let Some(task) = tasks.get_mut(task_id) {
            task.status = status;
            task.result = result;
            task.error = error;
            task.progress = progress;
            task.updated_at = Utc::now();
        }
    }

    /// List all capabilities across all local agents.
    pub async fn list_all_capabilities(&self) -> Vec<AgentCapabilities> {
        let registry = self.capability_registry.read().await;
        registry
            .iter()
            .map(|(agent_id, caps)| {
                AgentCapabilities {
                    agent_id: agent_id.clone(),
                    agent_name: agent_id.clone(), // Will be enriched with actual name
                    capabilities: caps.clone(),
                }
            })
            .collect()
    }

    /// List active tasks.
    pub async fn list_active_tasks(&self) -> Vec<TaskStatusResponse> {
        self.active_tasks.read().await.values().cloned().collect()
    }

    /// Build a CapabilitiesResponse (K2K format) for all local agents.
    pub async fn capabilities_response(&self, node_id: &str) -> CapabilitiesResponse {
        let registry = self.capability_registry.read().await;
        let all_caps: Vec<AgentCapability> =
            registry.values().flat_map(|caps| caps.clone()).collect();

        CapabilitiesResponse {
            node_id: node_id.to_string(),
            capabilities: all_caps,
            protocol_version: "1.0".to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenize_removes_stop_words() {
        let tokens = tokenize("I want to review the code for bugs");
        assert!(tokens.contains(&"review".to_string()));
        assert!(tokens.contains(&"code".to_string()));
        assert!(tokens.contains(&"bugs".to_string()));
        assert!(!tokens.contains(&"the".to_string()));
        assert!(!tokens.contains(&"to".to_string()));
        assert!(!tokens.contains(&"i".to_string()));
    }

    #[test]
    fn test_tokenize_handles_special_chars() {
        let tokens = tokenize("code-review, pull_request analysis!");
        assert!(tokens.contains(&"code-review".to_string()));
        assert!(tokens.contains(&"pull_request".to_string()));
        assert!(tokens.contains(&"analysis".to_string()));
    }

    #[test]
    fn test_score_exact_name_match() {
        let cap = AgentCapability {
            id: "test:code_review".to_string(),
            name: "code review".to_string(),
            category: CapabilityCategory::Skill,
            description: "Reviews code for bugs and style".to_string(),
            input_schema: None,
            version: "1.0".to_string(),
            min_protocol_version: None,
        };

        let query = tokenize("code review");
        let score = score_capability(&query, &cap);
        assert!(score > 1.0, "Exact name match should score high: {}", score);
    }

    #[test]
    fn test_score_partial_match() {
        let cap = AgentCapability {
            id: "test:web_research".to_string(),
            name: "web research".to_string(),
            category: CapabilityCategory::Skill,
            description: "Search the web and compile research reports".to_string(),
            input_schema: None,
            version: "1.0".to_string(),
            min_protocol_version: None,
        };

        let query = tokenize("research current trends in AI");
        let score = score_capability(&query, &cap);
        assert!(
            score > 0.0,
            "Partial match should have positive score: {}",
            score
        );
    }

    #[test]
    fn test_score_no_match() {
        let cap = AgentCapability {
            id: "test:cooking".to_string(),
            name: "cooking".to_string(),
            category: CapabilityCategory::Skill,
            description: "Prepare recipes and meal plans".to_string(),
            input_schema: None,
            version: "1.0".to_string(),
            min_protocol_version: None,
        };

        let query = tokenize("review my Rust code");
        let score = score_capability(&query, &cap);
        assert!(
            score < 0.1,
            "Unrelated capability should score near zero: {}",
            score
        );
    }

    #[test]
    fn test_specificity_scoring() {
        let specific_cap = AgentCapability {
            id: "test:code_review".to_string(),
            name: "code review".to_string(),
            category: CapabilityCategory::Skill,
            description: "Reviews code".to_string(),
            input_schema: None,
            version: "1.0".to_string(),
            min_protocol_version: None,
        };
        let generic_cap = AgentCapability {
            id: "test:general".to_string(),
            name: "general purpose assistant with many broad capabilities".to_string(),
            category: CapabilityCategory::Skill,
            description: "A general assistant that can code review and do many other things"
                .to_string(),
            input_schema: None,
            version: "1.0".to_string(),
            min_protocol_version: None,
        };

        let query = tokenize("code review");
        let specific_score = score_capability(&query, &specific_cap);
        let generic_score = score_capability(&query, &generic_cap);

        assert!(
            specific_score > generic_score,
            "Specific capability ({}) should score higher than generic ({})",
            specific_score,
            generic_score
        );
    }

    #[tokio::test]
    async fn test_find_best_agent() {
        let orch = AgentOrchestrator::new();

        orch.register_agent_capabilities(
            "code-agent",
            &[AgentCapabilityConfig {
                name: "code review".to_string(),
                category: "skill".to_string(),
                description: "Reviews code for bugs and style issues".to_string(),
            }],
        )
        .await;

        orch.register_agent_capabilities(
            "research-agent",
            &[AgentCapabilityConfig {
                name: "web research".to_string(),
                category: "knowledge".to_string(),
                description: "Searches the web and compiles research".to_string(),
            }],
        )
        .await;

        let result = orch.find_best_agent(None, "review my code for bugs").await;
        assert!(result.is_some());
        assert_eq!(result.unwrap().agent_id, "code-agent");

        let result = orch.find_best_agent(None, "research market trends").await;
        assert!(result.is_some());
        assert_eq!(result.unwrap().agent_id, "research-agent");
    }

    #[tokio::test]
    async fn test_no_match_returns_none() {
        let orch = AgentOrchestrator::new();

        orch.register_agent_capabilities(
            "code-agent",
            &[AgentCapabilityConfig {
                name: "code review".to_string(),
                category: "skill".to_string(),
                description: "Reviews code".to_string(),
            }],
        )
        .await;

        let result = orch.find_best_agent(None, "xyz qqq zzz").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_rank_agents() {
        let orch = AgentOrchestrator::new();

        orch.register_agent_capabilities(
            "code-agent",
            &[AgentCapabilityConfig {
                name: "code review".to_string(),
                category: "skill".to_string(),
                description: "Reviews code for bugs".to_string(),
            }],
        )
        .await;

        orch.register_agent_capabilities(
            "qa-agent",
            &[AgentCapabilityConfig {
                name: "bug detection".to_string(),
                category: "skill".to_string(),
                description: "Detects bugs in code through testing".to_string(),
            }],
        )
        .await;

        let ranked = orch
            .rank_agents_for_capability(None, "find bugs in code")
            .await;
        assert!(ranked.len() >= 1);
        // Both should match since they both deal with code/bugs
        for agent in &ranked {
            assert!(agent.score > 0.0);
        }
    }
}
