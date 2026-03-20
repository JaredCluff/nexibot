//! Workspace confinement for file tools.
//!
//! Restricts file operations to a workspace root directory
//! plus optional extra allowed directories.

use std::path::{Path, PathBuf};
use tracing::debug;

use super::path_validation::{validate_path_no_symlink_components, validate_path_within};

/// Paths that are always blocked from workspace access, relative to the home directory.
/// These protect sensitive credentials, keys, and browser profile data.
/// Platform-specific paths are included based on the current OS.
fn blocked_home_relative_paths() -> Vec<&'static str> {
    let mut paths = vec![
        // Common across all platforms
        ".ssh",
        ".gnupg",
        ".aws",
        ".kube",
    ];

    #[cfg(target_os = "macos")]
    {
        paths.extend_from_slice(&[
            ".mozilla",
            "Library/Application Support/Google/Chrome",
            "Library/Application Support/Firefox",
        ]);
    }

    #[cfg(target_os = "windows")]
    {
        paths.extend_from_slice(&[
            r"AppData\Local\Google\Chrome\User Data",
            r"AppData\Roaming\Mozilla\Firefox",
        ]);
    }

    #[cfg(target_os = "linux")]
    {
        paths.extend_from_slice(&[
            ".mozilla",
            ".config/google-chrome",
        ]);
    }

    paths
}

/// Configuration for workspace confinement.
#[derive(Debug, Clone)]
pub struct WorkspaceConfig {
    /// Root directory of the workspace (dedicated workspace dir, not home).
    pub root: PathBuf,
    /// Extra directories allowed outside the root.
    pub extra_allowed: Vec<PathBuf>,
    /// Paths that are explicitly blocked even if inside the workspace root.
    pub blocked_paths: Vec<PathBuf>,
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        let root = home.join(".config/nexibot/workspace");

        let blocked_paths = blocked_home_relative_paths()
            .iter()
            .map(|p| home.join(p))
            .collect();

        Self {
            root,
            extra_allowed: vec![],
            blocked_paths,
        }
    }
}

/// Resolve `..` and `.` components lexically without touching the filesystem.
///
/// Used as a fallback inside `is_blocked()` when `fs::canonicalize()` fails because
/// one or more path components do not yet exist.  Without this, a path like
/// `/nonexistent/../home/user/.ssh/id_rsa` would be returned as-is from
/// `canonicalize()`'s error fallback — and `.starts_with("/home/user/.ssh")` would
/// return `false`, silently bypassing the blocklist.
fn lexical_normalize(path: &Path) -> PathBuf {
    let mut components: Vec<std::path::Component> = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {} // skip '.'
            std::path::Component::ParentDir => {
                // Pop last Normal component; preserve RootDir/prefix when stack is empty
                match components.last() {
                    Some(std::path::Component::Normal(_)) => {
                        components.pop();
                    }
                    _ => {
                        components.push(component);
                    }
                }
            }
            other => components.push(other),
        }
    }
    components.iter().collect()
}

/// Check if a path is within or equal to a blocked path.
fn is_blocked(path: &Path, blocked_paths: &[PathBuf]) -> bool {
    let canonical =
        std::fs::canonicalize(path).unwrap_or_else(|_| lexical_normalize(path));
    for blocked in blocked_paths {
        let canonical_blocked =
            std::fs::canonicalize(blocked).unwrap_or_else(|_| lexical_normalize(blocked));
        if canonical.starts_with(&canonical_blocked) {
            return true;
        }
    }
    false
}

/// Validate that a path is within the workspace and not in a blocked directory.
///
/// Checks against the workspace root and any extra allowed directories,
/// then ensures the path is not in any blocked directory.
pub fn validate_workspace_path(path: &Path, config: &WorkspaceConfig) -> Result<PathBuf, String> {
    // Check blocked paths first (these override all allowances)
    if is_blocked(path, &config.blocked_paths) {
        debug!(
            "[WORKSPACE] Path '{}' is in a blocked directory",
            path.display()
        );
        return Err(format!(
            "Path '{}' is in a blocked directory (sensitive credentials or browser data)",
            path.display()
        ));
    }

    // Reject paths with symlink components before any containment check.
    // validate_path_within canonicalizes at check-time, but a symlink created
    // between validation and file-open (TOCTOU) could redirect the path outside
    // the workspace.  Rejecting symlink components up front — the same guard
    // applied to extra_allowed paths below — closes that window for the root
    // path check as well.
    if let Err(e) = validate_path_no_symlink_components(path) {
        debug!(
            "[WORKSPACE] Path '{}' contains symlink components, rejecting: {}",
            path.display(),
            e
        );
        return Err(format!(
            "Path '{}' contains symlink components (TOCTOU symlink escape blocked): {}",
            path.display(),
            e
        ));
    }

    // Try root first
    if let Ok(validated) = validate_path_within(path, &config.root) {
        return Ok(validated);
    }

    // Try each extra allowed directory.
    // Reject any extra_allowed entry that itself contains symlink components: a symlink
    // placed inside an extra_allowed dir could point anywhere on the filesystem, bypassing
    // workspace confinement entirely.  We also validate the target path for symlink
    // components before accepting it through an extra_allowed directory.
    for extra in &config.extra_allowed {
        // Skip extra_allowed dirs that contain symlink components — they cannot be
        // trusted as a confinement boundary because the symlink could be redirected
        // to point outside the intended directory at any time.
        if let Err(e) = validate_path_no_symlink_components(extra) {
            tracing::warn!(
                "[WORKSPACE] extra_allowed dir '{}' contains symlink components, skipping: {}",
                extra.display(),
                e
            );
            continue;
        }
        // Also reject the target path if any of its existing components are symlinks.
        if let Err(e) = validate_path_no_symlink_components(path) {
            debug!(
                "[WORKSPACE] Path '{}' contains symlink components, rejecting for extra_allowed check: {}",
                path.display(),
                e
            );
            return Err(format!(
                "Path '{}' contains symlink components (symlink escape via extra_allowed blocked): {}",
                path.display(),
                e
            ));
        }
        if let Ok(validated) = validate_path_within(path, extra) {
            return Ok(validated);
        }
    }

    debug!(
        "[WORKSPACE] Path '{}' is not within workspace (root: {}, extras: {})",
        path.display(),
        config.root.display(),
        config.extra_allowed.len()
    );

    Err(format!(
        "Path '{}' is outside the workspace. Allowed: {} + {} extra directories",
        path.display(),
        config.root.display(),
        config.extra_allowed.len()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workspace_default_root_is_dedicated_dir() {
        let config = WorkspaceConfig::default();
        let root_str = config.root.to_string_lossy();
        // Normalize separators for cross-platform comparison
        let normalized = root_str.replace('\\', "/");
        assert!(
            normalized.contains(".config/nexibot/workspace"),
            "Default root should be dedicated workspace dir, got: {}",
            root_str
        );
    }

    #[test]
    fn test_workspace_default_has_blocked_paths() {
        let config = WorkspaceConfig::default();
        assert!(
            !config.blocked_paths.is_empty(),
            "Default should have blocked paths"
        );
        if let Some(_home) = dirs::home_dir() {
            let blocked_strs: Vec<String> = config
                .blocked_paths
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect();
            // Common paths present on all platforms
            assert!(blocked_strs.iter().any(|p| p.ends_with(".ssh")));
            assert!(blocked_strs.iter().any(|p| p.ends_with(".gnupg")));
            assert!(blocked_strs.iter().any(|p| p.ends_with(".aws")));
            assert!(blocked_strs.iter().any(|p| p.ends_with(".kube")));
        }
    }

    #[test]
    fn test_workspace_blocks_outside() {
        let tmp_dir = std::env::temp_dir();
        let tmp = std::fs::canonicalize(&tmp_dir).unwrap_or(tmp_dir);
        let config = WorkspaceConfig {
            root: tmp.join("workspace"),
            extra_allowed: vec![],
            blocked_paths: vec![],
        };
        // Use a path that definitely doesn't start with the temp dir
        let outside = if cfg!(windows) {
            PathBuf::from("C:\\Windows\\System32\\drivers\\etc\\hosts")
        } else {
            PathBuf::from("/etc/passwd")
        };
        assert!(validate_workspace_path(&outside, &config).is_err());
    }

    #[test]
    fn test_workspace_extra_allowed() {
        let tmp_dir = std::env::temp_dir();
        let tmp = std::fs::canonicalize(&tmp_dir).unwrap_or(tmp_dir);
        let config = WorkspaceConfig {
            root: tmp.join("workspace"),
            extra_allowed: vec![tmp.clone()],
            blocked_paths: vec![],
        };
        let path = tmp.join("allowed.txt");
        assert!(validate_workspace_path(&path, &config).is_ok());
    }

    #[test]
    fn test_workspace_blocks_ssh_dir() {
        let tmp_dir = std::env::temp_dir();
        let tmp = std::fs::canonicalize(&tmp_dir).unwrap_or(tmp_dir);
        let ssh_dir = tmp.join("fakehome/.ssh");
        let config = WorkspaceConfig {
            root: tmp.join("fakehome"),
            extra_allowed: vec![],
            blocked_paths: vec![ssh_dir.clone()],
        };
        let path = tmp.join("fakehome/.ssh/id_rsa");
        assert!(validate_workspace_path(&path, &config).is_err());
    }
}
