//! LLM-based workflow planner.
//!
//! Given a natural language goal, queries K2K for available capabilities,
//! builds a planning prompt, calls the LLM, and parses the resulting
//! WorkflowSpec JSON.

use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::RwLock;
use tracing::info;
use uuid::Uuid;

use super::workflow_spec::{WorkflowSpec, WorkflowStep};
use crate::k2k_client::K2KIntegration;

// ── Planner ──────────────────────────────────────────────────────────────────

/// Plans workflows from natural language goals using the LLM.
pub struct AgentPlanner {
    claude: Arc<RwLock<crate::claude::ClaudeClient>>,
    k2k: Arc<RwLock<K2KIntegration>>,
}

impl AgentPlanner {
    /// Create a new planner.
    pub fn new(
        claude: Arc<RwLock<crate::claude::ClaudeClient>>,
        k2k: Arc<RwLock<K2KIntegration>>,
    ) -> Self {
        Self { claude, k2k }
    }

    /// Given a natural language goal, generate a `WorkflowSpec`.
    ///
    /// Steps:
    /// 1. Discover available capabilities from K2K (graceful fallback on error).
    /// 2. Build a planning prompt that includes capability descriptions.
    /// 3. Call the LLM and extract a JSON block from the response.
    /// 4. Parse and validate the `WorkflowSpec`.
    pub async fn plan(&self, goal: &str) -> Result<WorkflowSpec> {
        // 1. Discover capabilities
        let capabilities = self.list_capabilities().await;

        // 2. Build prompt
        let prompt = build_planning_prompt(goal, &capabilities);
        info!(
            "[PLANNER] Planning workflow for goal: '{}' ({} capabilities available)",
            goal,
            capabilities.len()
        );

        // 3. Call LLM
        let claude = self.claude.read().await;
        let response = claude
            .send_message(&prompt)
            .await
            .context("Planner: LLM call failed")?;
        drop(claude);

        // 4. Extract JSON and parse
        let json_str = extract_json(&response)
            .ok_or_else(|| anyhow::anyhow!("Planner: LLM response contained no JSON block"))?;

        let mut spec: WorkflowSpec = serde_json::from_str(json_str)
            .context("Planner: failed to parse WorkflowSpec from LLM response")?;

        // Ensure spec has an id
        if spec.id.is_empty() {
            spec.id = Uuid::new_v4().to_string();
        }

        validate_spec(&spec, &capabilities)?;

        info!(
            "[PLANNER] Generated workflow '{}' with {} steps",
            spec.name,
            spec.steps.len()
        );
        Ok(spec)
    }

    // ── Capability listing ────────────────────────────────────────────────────

    async fn list_capabilities(&self) -> Vec<CapabilityDescription> {
        // Always include built-in local capabilities.
        let mut caps = built_in_capabilities();

        // Attempt to fetch additional capabilities from K2K.
        let k2k = self.k2k.read().await;
        if k2k.is_available().await {
            let config = k2k.get_config().await;
            let base_url = config.k2k.local_agent_url.clone();
            drop(config);

            let client_guard = k2k.get_client().await;
            if let Some(client) = client_guard.as_ref() {
                match client.list_capabilities(&base_url).await {
                    Ok(resp) => {
                        for c in resp.capabilities {
                            caps.push(CapabilityDescription {
                                id: c.id.clone(),
                                name: c.name.clone(),
                                description: c.description.clone(),
                            });
                        }
                    }
                    Err(e) => {
                        tracing::warn!("[PLANNER] Could not fetch K2K capabilities: {}", e);
                    }
                }
            }
        }

        caps
    }
}

// ── Capability description (for prompt building) ──────────────────────────────

#[derive(Debug, Clone)]
pub struct CapabilityDescription {
    pub id: String,
    pub name: String,
    pub description: String,
}

/// Built-in local capabilities always available without K2K.
fn built_in_capabilities() -> Vec<CapabilityDescription> {
    vec![
        CapabilityDescription {
            id: "llm.complete".to_string(),
            name: "LLM Completion".to_string(),
            description: "Generate text with the local LLM. Input: {\"prompt\": \"...\"}. Output: {\"text\": \"...\"}".to_string(),
        },
        CapabilityDescription {
            id: "llm.embed".to_string(),
            name: "LLM Embedding".to_string(),
            description: "Generate a vector embedding for text. Input: {\"text\": \"...\"}".to_string(),
        },
        CapabilityDescription {
            id: "kb.read".to_string(),
            name: "Knowledge Base Read".to_string(),
            description: "Query the knowledge base for relevant documents. Input: {\"query\": \"...\", \"top_k\": 10}. Output: {\"results\": [...]}".to_string(),
        },
        CapabilityDescription {
            id: "kb.write".to_string(),
            name: "Knowledge Base Write".to_string(),
            description: "Save a new article to the knowledge base. Input: {\"title\": \"...\", \"content\": \"...\", \"tags\": [...]}".to_string(),
        },
        CapabilityDescription {
            id: "code.execute".to_string(),
            name: "Code Execution (Sandbox)".to_string(),
            description: "Execute code in a sandboxed container. Input: {\"code\": \"...\", \"language\": \"python|bash|node\"}. Output: {\"stdout\": \"...\", \"stderr\": \"...\", \"exit_code\": 0}".to_string(),
        },
        CapabilityDescription {
            id: "http.get".to_string(),
            name: "HTTP GET".to_string(),
            description: "Perform an HTTP GET request. Input: {\"url\": \"https://...\"}. Output: {\"status\": 200, \"body\": ...}".to_string(),
        },
        CapabilityDescription {
            id: "http.post".to_string(),
            name: "HTTP POST".to_string(),
            description: "Perform an HTTP POST request with a JSON body. Input: {\"url\": \"https://...\", \"body\": {...}}. Output: {\"status\": 200, \"body\": ...}".to_string(),
        },
    ]
}

// ── Prompt builder ────────────────────────────────────────────────────────────

fn build_planning_prompt(goal: &str, capabilities: &[CapabilityDescription]) -> String {
    let caps_list: String = capabilities
        .iter()
        .map(|c| format!("  - **{}** (`{}`): {}", c.name, c.id, c.description))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        r#"You are a workflow planner. Your job is to translate the user's goal into a structured WorkflowSpec JSON object.

## Available Capabilities
{caps}

## WorkflowSpec schema
```json
{{
  "id": "<uuid or empty string>",
  "name": "<short name for the workflow>",
  "description": "<one sentence description>",
  "steps": [
    {{
      "id": "<step_id>",
      "capability": "<capability_id>",
      "input": {{ /* input fields for the capability */ }},
      "depends_on": ["<step_id>", ...],
      "condition": "<optional expression using {{{{var}}}} substitution>",
      "output_var": "<optional variable name to store step output>",
      "parallel": false,
      "loop_over": null,
      "on_failure": {{ "action": "abort|skip|retry", "max_retries": 0 }}
    }}
  ],
  "inputs": {{ /* initial input variables */ }}
}}
```

## Rules
- Use only capabilities from the list above.
- Steps that can run independently should have `"parallel": true`.
- Use `output_var` to pass data between steps via `{{{{variable_name}}}}` substitution in downstream `input` fields.
- Set `on_failure.action` to `"skip"` for optional steps, `"retry"` for transient failures, `"abort"` for critical steps.
- Respond with ONLY the JSON object, wrapped in a ```json ... ``` code block. No other text.

## Goal
{goal}

Respond with the WorkflowSpec JSON now."#,
        caps = caps_list,
        goal = goal
    )
}

// ── JSON extraction ───────────────────────────────────────────────────────────

/// Extract the content of the first ```json ... ``` block in an LLM response.
/// Falls back to extracting the first `{...}` balanced object if no fenced block exists.
pub fn extract_json(text: &str) -> Option<&str> {
    // Try fenced code block first
    if let Some(start) = text.find("```json") {
        let after = &text[start + 7..];
        if let Some(end) = after.find("```") {
            return Some(after[..end].trim());
        }
    }
    // Try bare ``` block
    if let Some(start) = text.find("```") {
        let after = &text[start + 3..];
        if let Some(end) = after.find("```") {
            let candidate = after[..end].trim();
            if candidate.starts_with('{') {
                return Some(candidate);
            }
        }
    }
    // Fall back: find the first balanced `{...}` in the response
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut start_idx = None;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'{' => {
                if depth == 0 {
                    start_idx = Some(i);
                }
                depth += 1;
            }
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    if let Some(s) = start_idx {
                        return Some(&text[s..=i]);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

// ── Spec validation ───────────────────────────────────────────────────────────

/// Validate a parsed WorkflowSpec against the available capabilities.
///
/// Checks:
/// - At least one step exists.
/// - All step IDs are unique.
/// - All `depends_on` references point to existing step IDs.
/// - All capability IDs are in the provided list.
pub fn validate_spec(
    spec: &WorkflowSpec,
    capabilities: &[CapabilityDescription],
) -> Result<()> {
    if spec.steps.is_empty() {
        anyhow::bail!("WorkflowSpec has no steps");
    }

    let known_ids: std::collections::HashSet<&str> =
        spec.steps.iter().map(|s| s.id.as_str()).collect();

    if known_ids.len() != spec.steps.len() {
        anyhow::bail!("WorkflowSpec contains duplicate step IDs");
    }

    let known_caps: std::collections::HashSet<&str> =
        capabilities.iter().map(|c| c.id.as_str()).collect();

    for step in &spec.steps {
        // Validate dependency references
        for dep in &step.depends_on {
            if !known_ids.contains(dep.as_str()) {
                anyhow::bail!(
                    "Step '{}' depends_on unknown step '{}'",
                    step.id,
                    dep
                );
            }
        }

        // Validate capability IDs (warn but don't fail for unknown K2K caps)
        if !known_caps.contains(step.capability.as_str()) {
            tracing::warn!(
                "[PLANNER] Step '{}' uses unknown capability '{}' — will fall through to K2K",
                step.id,
                step.capability
            );
        }
    }

    // Detect circular dependencies via DFS
    detect_cycles(spec)?;

    Ok(())
}

fn detect_cycles(spec: &WorkflowSpec) -> Result<()> {
    use std::collections::HashMap;

    let mut state: HashMap<&str, u8> = HashMap::new(); // 0=unvisited,1=visiting,2=done

    fn dfs<'a>(
        id: &'a str,
        steps: &'a [WorkflowStep],
        state: &mut HashMap<&'a str, u8>,
    ) -> bool {
        match state.get(id).copied().unwrap_or(0) {
            1 => return true, // cycle
            2 => return false, // already done
            _ => {}
        }
        state.insert(id, 1);
        if let Some(step) = steps.iter().find(|s| s.id == id) {
            for dep in &step.depends_on {
                if dfs(dep, steps, state) {
                    return true;
                }
            }
        }
        state.insert(id, 2);
        false
    }

    for step in &spec.steps {
        if dfs(&step.id, &spec.steps, &mut state) {
            anyhow::bail!(
                "WorkflowSpec contains a circular dependency involving step '{}'",
                step.id
            );
        }
    }

    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::workflow_spec::{FailureAction, OnFailure};
    use std::collections::HashMap;

    fn simple_spec(steps: Vec<WorkflowStep>) -> WorkflowSpec {
        WorkflowSpec {
            id: "test".to_string(),
            name: "Test".to_string(),
            description: None,
            steps,
            inputs: HashMap::new(),
        }
    }

    fn step(id: &str, cap: &str, deps: Vec<&str>) -> WorkflowStep {
        WorkflowStep {
            id: id.to_string(),
            capability: cap.to_string(),
            input: serde_json::Value::Null,
            depends_on: deps.into_iter().map(str::to_string).collect(),
            condition: None,
            output_var: None,
            parallel: false,
            loop_over: None,
            on_failure: OnFailure {
                action: FailureAction::Abort,
                max_retries: 0,
            },
        }
    }

    #[test]
    fn test_validate_ok() {
        let caps = vec![CapabilityDescription {
            id: "llm.complete".to_string(),
            name: "LLM".to_string(),
            description: "".to_string(),
        }];
        let spec = simple_spec(vec![
            step("a", "llm.complete", vec![]),
            step("b", "llm.complete", vec!["a"]),
        ]);
        assert!(validate_spec(&spec, &caps).is_ok());
    }

    #[test]
    fn test_validate_missing_dep() {
        let caps = vec![CapabilityDescription {
            id: "llm.complete".to_string(),
            name: "LLM".to_string(),
            description: "".to_string(),
        }];
        let spec = simple_spec(vec![step("b", "llm.complete", vec!["nonexistent"])]);
        assert!(validate_spec(&spec, &caps).is_err());
    }

    #[test]
    fn test_validate_cycle() {
        let caps = vec![CapabilityDescription {
            id: "llm.complete".to_string(),
            name: "LLM".to_string(),
            description: "".to_string(),
        }];
        let spec = simple_spec(vec![
            step("a", "llm.complete", vec!["b"]),
            step("b", "llm.complete", vec!["a"]),
        ]);
        assert!(validate_spec(&spec, &caps).is_err());
    }

    #[test]
    fn test_extract_json_fenced() {
        let text = "Here is the plan:\n```json\n{\"id\":\"x\"}\n```\nDone.";
        assert_eq!(extract_json(text), Some("{\"id\":\"x\"}"));
    }

    #[test]
    fn test_extract_json_bare() {
        let text = "Result: {\"steps\": []} end";
        assert!(extract_json(text).is_some());
    }
}
