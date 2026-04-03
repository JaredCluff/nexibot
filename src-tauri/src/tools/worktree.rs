use crate::tool_registry::{PermissionDecision, Tool, ToolContext, ToolResult};
use async_trait::async_trait;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Default)]
pub struct WorktreeState {
    pub active: bool,
    pub original_cwd: PathBuf,
    pub worktree_path: PathBuf,
    pub branch: String,
    pub original_branch: String,
}

pub struct WorktreeTool {
    pub state: Arc<RwLock<WorktreeState>>,
}

#[async_trait]
impl Tool for WorktreeTool {
    fn name(&self) -> &str { "nexibot_worktree" }
    fn description(&self) -> &str {
        "Manage git worktrees for isolated code changes. Actions: enter (create isolated worktree), exit (return to main), status (show current worktree info)."
    }
    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["enter", "exit", "status"] },
                "name": { "type": "string", "description": "Worktree name (required for enter)" },
                "discard_changes": { "type": "boolean", "description": "Required to remove worktree with uncommitted changes (default false)" }
            },
            "required": ["action"]
        })
    }
    async fn check_permissions(&self, input: &Value, _ctx: &ToolContext) -> PermissionDecision {
        match input["action"].as_str() {
            Some("exit") => {
                let state = self.state.read().await;
                if state.active {
                    PermissionDecision::Ask {
                        reason: "Exit worktree and return to original working directory?".to_string(),
                        details: Some(format!("Worktree branch: {}", state.branch)),
                    }
                } else {
                    PermissionDecision::Allow
                }
            }
            _ => PermissionDecision::Allow,
        }
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> ToolResult {
        let action = match input["action"].as_str() {
            Some(a) => a,
            None => return ToolResult::err("action is required"),
        };
        match action {
            "enter" => self.enter(input, &ctx).await,
            "exit" => self.exit(input, &ctx).await,
            "status" => self.status(&ctx).await,
            _ => ToolResult::err(format!("Unknown action: {}", action)),
        }
    }
}

impl WorktreeTool {
    async fn enter(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        {
            let state = self.state.read().await;
            if state.active {
                return ToolResult::err(format!(
                    "Already in a worktree session (branch: {}); call exit first",
                    state.branch
                ));
            }
        }

        let name = match input["name"].as_str() {
            Some(n) => sanitize_slug(n),
            None => return ToolResult::err("name is required for enter"),
        };
        if name.is_empty() {
            return ToolResult::err("name must contain at least one alphanumeric character");
        }

        let git_root = match find_git_root(&ctx.working_dir).await {
            Some(r) => r,
            None => return ToolResult::err("Not in a git repository"),
        };

        let worktree_path = git_root.join(".nexibot").join("worktrees").join(&name);
        let branch = format!("worktree-{}", name);

        // Create the worktree
        let result = run_git(
            &git_root,
            &["worktree", "add", "-B", &branch,
              &worktree_path.to_string_lossy(), "HEAD"]
        ).await;

        if let Err(e) = result {
            return ToolResult::err(format!("Failed to create worktree: {}", e));
        }

        let original_branch = run_git(&git_root, &["branch", "--show-current"])
            .await.unwrap_or_default().trim().to_string();

        let mut state = self.state.write().await;
        *state = WorktreeState {
            active: true,
            original_cwd: ctx.working_dir.clone(),
            worktree_path: worktree_path.clone(),
            branch: branch.clone(),
            original_branch,
        };

        ToolResult::ok(format!(
            "Entered worktree '{}'\nPath: {}\nBranch: {}\n\nYour changes in this worktree are isolated from the main branch.",
            name, worktree_path.display(), branch
        ))
    }

    async fn exit(&self, input: Value, _ctx: &ToolContext) -> ToolResult {
        let discard = input["discard_changes"].as_bool().unwrap_or(false);
        let state = self.state.read().await.clone();

        if !state.active {
            return ToolResult::ok("No active worktree session.");
        }

        // Check for uncommitted changes
        let has_changes = check_worktree_changes(&state.worktree_path).await;

        if has_changes && !discard {
            return ToolResult::err(format!(
                "Worktree '{}' has uncommitted changes.\n\
                 To keep the worktree (return to it later): just note the path: {}\n\
                 To discard all changes: set discard_changes=true",
                state.branch, state.worktree_path.display()
            ));
        }

        if discard {
            // Remove the worktree
            let git_root = find_git_root(&state.original_cwd).await
                .unwrap_or(state.original_cwd.clone());
            let _ = run_git(&git_root, &[
                "worktree", "remove", "--force",
                &state.worktree_path.to_string_lossy()
            ]).await;
            // Brief pause to allow the kernel to flush the directory entry before branch deletion.
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            // Delete the branch (but not if it's main/master)
            if !is_protected_branch(&state.branch) {
                let _ = run_git(&git_root, &["branch", "-D", &state.branch]).await;
            }
        }

        let original = state.original_cwd.clone();
        *self.state.write().await = WorktreeState::default();

        ToolResult::ok(format!(
            "Exited worktree. Working directory restored to: {}",
            original.display()
        ))
    }

    async fn status(&self, _ctx: &ToolContext) -> ToolResult {
        let state = self.state.read().await;
        if !state.active {
            return ToolResult::ok("No active worktree session.");
        }
        let changes = check_worktree_changes(&state.worktree_path).await;
        ToolResult::ok(format!(
            "Active worktree:\n  Branch: {}\n  Path: {}\n  Has changes: {}",
            state.branch,
            state.worktree_path.display(),
            if changes { "yes" } else { "no (clean)" }
        ))
    }
}

// ─── Sub-agent worktree creation (called from orchestration) ─────────────────

/// Create an isolated worktree for a sub-agent. Does NOT modify global session state.
pub async fn create_agent_worktree(
    git_root: &Path,
    agent_id: &str,
) -> anyhow::Result<PathBuf> {
    let slug = format!("agent-{}", &agent_id[..8.min(agent_id.len())]);
    let worktree_path = git_root.join(".nexibot").join("worktrees").join(&slug);
    let branch = format!("worktree-{}", slug);

    run_git(git_root, &[
        "worktree", "add", "-B", &branch,
        &worktree_path.to_string_lossy(), "HEAD"
    ]).await?;

    Ok(worktree_path)
}

/// Remove an agent worktree. Requires git_root to be passed explicitly
/// (since the worktree directory is being deleted).
// TODO: Call from subagent_executor when a Worktree-isolated agent completes or is cancelled.
#[allow(dead_code)]
pub async fn remove_agent_worktree(git_root: &Path, worktree_path: &Path, branch: &str) {
    let _ = run_git(git_root, &[
        "worktree", "remove", "--force",
        &worktree_path.to_string_lossy()
    ]).await;
    // Brief pause to allow the kernel to flush the directory entry before branch deletion.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    if !is_protected_branch(branch) {
        let _ = run_git(git_root, &["branch", "-D", branch]).await;
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

pub async fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    loop {
        if current.join(".git").exists() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

fn sanitize_slug(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn is_protected_branch(branch: &str) -> bool {
    matches!(branch, "main" | "master" | "develop" | "dev")
}

async fn check_worktree_changes(path: &Path) -> bool {
    if !path.exists() { return false; }
    run_git(path, &["status", "--porcelain"])
        .await
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
}

async fn run_git(dir: &Path, args: &[&str]) -> anyhow::Result<String> {
    let output = tokio::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .await?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(anyhow::anyhow!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_slug_replaces_spaces() {
        assert_eq!(sanitize_slug("my feature"), "my-feature");
    }

    #[test]
    fn test_sanitize_slug_removes_leading_trailing_dashes() {
        assert_eq!(sanitize_slug("--feature--"), "feature");
    }

    #[test]
    fn test_is_protected_branch() {
        assert!(is_protected_branch("main"));
        assert!(is_protected_branch("master"));
        assert!(!is_protected_branch("worktree-my-feature"));
    }

    #[tokio::test]
    async fn test_find_git_root_finds_repo() {
        // The nexibot repo itself is a git repo, so this should find the root
        let cwd = std::env::current_dir().unwrap();
        let root = find_git_root(&cwd).await;
        assert!(root.is_some());
    }

    #[tokio::test]
    async fn test_find_git_root_returns_none_outside_repo() {
        let root = find_git_root(Path::new("/tmp")).await;
        // /tmp is not inside a git repo on macOS/Linux in standard configurations
        assert!(root.is_none());
    }

    #[tokio::test]
    async fn test_worktree_status_when_inactive() {
        let state = Arc::new(RwLock::new(WorktreeState::default()));
        let tool = WorktreeTool { state };
        let ctx = ToolContext {
            session_key: "s".into(), agent_id: "a".into(),
            working_dir: PathBuf::from("/tmp"),
        };
        let result = tool.call(serde_json::json!({"action": "status"}), ctx).await;
        assert!(result.success);
        assert!(result.content.contains("No active worktree"));
    }
}
