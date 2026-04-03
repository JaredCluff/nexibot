use std::path::Path;
use tokio::process::Command as AsyncCommand;

#[derive(Debug, Clone, Default)]
pub struct GitContext {
    pub branch: String,
    pub recent_commits: String,
    pub status: String,
    pub user_name: String,
    pub tracking_branch: String,
}

impl GitContext {
    /// Format for system prompt injection.
    pub fn to_prompt_string(&self) -> String {
        if self.branch.is_empty() {
            return String::new(); // Not a git repo
        }
        let mut s = String::new();
        s.push_str(&format!("Current branch: {}\n", self.branch));
        if !self.tracking_branch.is_empty() && !self.tracking_branch.contains("no upstream") {
            s.push_str(&format!("Tracking: {}\n", self.tracking_branch));
        }
        if !self.user_name.is_empty() {
            s.push_str(&format!("Git user: {}\n", self.user_name));
        }
        if self.status.is_empty() {
            s.push_str("Status: (clean)\n");
        } else {
            s.push_str("Status:\n");
            s.push_str(&self.status);
            s.push('\n');
        }
        if !self.recent_commits.is_empty() {
            s.push_str("\nRecent commits:\n");
            s.push_str(&self.recent_commits);
            s.push('\n');
        }
        s
    }

    pub fn is_empty(&self) -> bool {
        self.branch.is_empty()
    }
}

/// Async version: runs 5 git commands in parallel using tokio.
/// Silently ignores errors (non-git directories return empty strings).
pub async fn collect_git_context(working_dir: &Path) -> GitContext {
    let dir = working_dir.to_path_buf();

    let (branch, log, status, user, tracking) = tokio::join!(
        run_git_async(&dir, &["branch", "--show-current"]),
        run_git_async(&dir, &["log", "--oneline", "-n", "5"]),
        run_git_async(&dir, &["status", "--short"]),
        run_git_async(&dir, &["config", "user.name"]),
        run_git_async(&dir, &["rev-parse", "--abbrev-ref", "@{u}"]),
    );

    let mut status_trimmed = status.trim().to_string();
    if status_trimmed.len() > 2000 {
        status_trimmed.truncate(2000);
        status_trimmed.push_str("\n... (truncated, run git status for full output)");
    }

    GitContext {
        branch: branch.trim().to_string(),
        recent_commits: log.trim().to_string(),
        status: status_trimmed,
        user_name: user.trim().to_string(),
        tracking_branch: tracking.trim().to_string(),
    }
}

/// Sync version: runs git commands using std::process::Command.
/// Used in synchronous system-prompt assembly context.
pub fn collect_git_context_sync(working_dir: &Path) -> GitContext {
    let branch = run_git_sync(working_dir, &["branch", "--show-current"]);
    if branch.trim().is_empty() {
        return GitContext::default(); // Not a git repo
    }
    let log = run_git_sync(working_dir, &["log", "--oneline", "-n", "5"]);
    let status = run_git_sync(working_dir, &["status", "--short"]);
    let user = run_git_sync(working_dir, &["config", "user.name"]);
    let tracking = run_git_sync(working_dir, &["rev-parse", "--abbrev-ref", "@{u}"]);

    let mut status_trimmed = status.trim().to_string();
    if status_trimmed.len() > 2000 {
        status_trimmed.truncate(2000);
        status_trimmed.push_str("\n... (truncated)");
    }

    GitContext {
        branch: branch.trim().to_string(),
        recent_commits: log.trim().to_string(),
        status: status_trimmed,
        user_name: user.trim().to_string(),
        tracking_branch: tracking.trim().to_string(),
    }
}

async fn run_git_async(dir: &Path, args: &[&str]) -> String {
    AsyncCommand::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .await
        .ok()
        .and_then(|o| if o.status.success() {
            String::from_utf8(o.stdout).ok()
        } else {
            None
        })
        .unwrap_or_default()
}

fn run_git_sync(dir: &Path, args: &[&str]) -> String {
    std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .ok()
        .and_then(|o| if o.status.success() {
            String::from_utf8(o.stdout).ok()
        } else {
            None
        })
        .unwrap_or_default()
}

/// Safety rules to inject into the system prompt for git operations.
pub const GIT_SAFETY_RULES: &str = "\
## Git Safety Rules
- NEVER update git config
- NEVER run destructive git commands (push --force, reset --hard, checkout ., clean -f, branch -D) without explicit user request
- NEVER skip hooks (--no-verify, --no-gpg-sign) unless the user explicitly asks
- NEVER force push to main or master — warn the user instead
- Always create NEW commits rather than amending, unless user explicitly requests amend
- Don't commit files that likely contain secrets (.env, credentials.json, *.key, *.pem)
- NEVER use -i flag for interactive git commands (not supported in this context)
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_prompt_string_empty_when_no_branch() {
        let ctx = GitContext::default();
        assert!(ctx.to_prompt_string().is_empty());
    }

    #[test]
    fn test_to_prompt_string_clean_repo() {
        let ctx = GitContext {
            branch: "main".to_string(),
            status: String::new(),
            recent_commits: "abc123 fix bug".to_string(),
            user_name: "Alice".to_string(),
            tracking_branch: String::new(),
        };
        let s = ctx.to_prompt_string();
        assert!(s.contains("Current branch: main"));
        assert!(s.contains("(clean)"));
        assert!(s.contains("Recent commits:"));
        assert!(s.contains("abc123 fix bug"));
    }

    #[test]
    fn test_to_prompt_string_with_status() {
        let ctx = GitContext {
            branch: "feat".to_string(),
            status: "M  src/main.rs".to_string(),
            ..Default::default()
        };
        let s = ctx.to_prompt_string();
        assert!(s.contains("M  src/main.rs"));
    }

    #[test]
    fn test_is_empty() {
        let empty = GitContext::default();
        assert!(empty.is_empty());
        let nonempty = GitContext { branch: "main".to_string(), ..Default::default() };
        assert!(!nonempty.is_empty());
    }

    #[test]
    fn test_git_safety_rules_not_empty() {
        assert!(!GIT_SAFETY_RULES.is_empty());
        assert!(GIT_SAFETY_RULES.contains("force push"));
    }
}
