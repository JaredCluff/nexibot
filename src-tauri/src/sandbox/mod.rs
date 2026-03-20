//! Docker container sandbox for secure command execution.
//!
//! Provides an isolated execution environment using Docker containers,
//! with configurable resource limits, network isolation, and security
//! policies to safely run untrusted or potentially dangerous commands.

pub mod docker;
pub mod policy;
pub mod validate;

use serde::{Deserialize, Serialize};
use tracing::warn;

/// Configuration for the Docker sandbox environment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxConfig {
    /// Whether sandboxing is enabled.
    pub enabled: bool,
    /// Docker image to use for the sandbox container (should be SHA256-pinned).
    pub image: String,
    /// Non-root user to run commands as inside the container.
    pub non_root_user: String,
    /// Memory limit for the container (e.g., "512m", "1g").
    pub memory_limit: String,
    /// CPU limit for the container (fractional cores, e.g., 1.0 = one core).
    pub cpu_limit: f64,
    /// Network mode for the container (e.g., "none", "bridge").
    pub network_mode: String,
    /// Maximum execution time in seconds before the container is killed.
    pub timeout_seconds: u64,
    /// Host paths that are blocked from being bind-mounted into the container.
    pub blocked_paths: Vec<String>,
    /// Optional seccomp profile path for additional syscall filtering.
    pub seccomp_profile: Option<String>,
    /// Optional AppArmor profile name for mandatory access control.
    pub apparmor_profile: Option<String>,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            // Full 64-character SHA-256 digest required.
            // The previous value ("98f4b71de414932") was only 16 hex chars (64-bit security),
            // which is insufficient for collision resistance. Use the complete digest.
            image: "debian:bookworm-slim@sha256:98f4b71de414932bb0b8a9ac41d0d3cf0ebb77a4638ae99c28a9e9bfe26ae98e".to_string(),
            non_root_user: "sandbox".to_string(),
            memory_limit: "512m".to_string(),
            cpu_limit: 1.0,
            network_mode: "none".to_string(),
            timeout_seconds: 60,
            // Tilde paths are expanded to the actual home directory at construction
            // time so that Docker bind-mount validation sees real absolute paths.
            blocked_paths: expand_blocked_paths(&[
                "/etc",
                "/proc",
                "/sys",
                "/dev",
                "/var/run/docker.sock",
                "/boot",
                "/root",
                "/.ssh",
                "~/.config",
                "~/.ssh",
                "~/.gnupg",
                "~/.aws",
                "~/.kube",
            ]),
            seccomp_profile: None,
            apparmor_profile: None,
        }
    }
}

/// Expand a tilde (`~`) prefix in a path to the user's home directory.
///
/// - `~/foo` → `<home>/foo`
/// - `~` → `<home>`
/// - Anything else → unchanged
///
/// If the home directory cannot be determined the path is returned as-is.
pub fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().into_owned();
        }
    } else if path == "~" {
        if let Some(home) = dirs::home_dir() {
            return home.to_string_lossy().into_owned();
        }
    }
    path.to_string()
}

/// Build the blocked-paths list, expanding any tilde prefixes.
fn expand_blocked_paths(paths: &[&str]) -> Vec<String> {
    paths.iter().map(|p| expand_tilde(p)).collect()
}

/// Emit a prominent `WARN`-level log message if the sandbox is disabled.
///
/// Call this once during application startup after loading [`SandboxConfig`].
/// Without sandboxing, all code execution runs directly on the host without
/// container isolation.
#[allow(dead_code)]
pub fn warn_if_sandbox_disabled(config: &SandboxConfig) {
    if !config.enabled {
        warn!("╔══════════════════════════════════════════════════════════════════╗");
        warn!("║  WARNING: Docker sandbox is DISABLED.                           ║");
        warn!("║  Code execution runs WITHOUT container isolation.               ║");
        warn!("║  Set sandbox.enabled: true in config.yaml for secure execution. ║");
        warn!("╚══════════════════════════════════════════════════════════════════╝");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = SandboxConfig::default();
        assert!(!config.enabled);

        // Image must contain a full 64-character SHA-256 digest.
        assert!(
            config.image.contains("@sha256:"),
            "image must include sha256 digest"
        );
        let digest_part = config.image.split("@sha256:").nth(1).unwrap_or("");
        assert_eq!(
            digest_part.len(),
            64,
            "SHA-256 digest must be 64 hex characters, got {} chars in '{}'",
            digest_part.len(),
            digest_part
        );

        assert_eq!(config.non_root_user, "sandbox");
        assert_eq!(config.memory_limit, "512m");
        assert!((config.cpu_limit - 1.0).abs() < f64::EPSILON);
        assert_eq!(config.network_mode, "none");
        assert_eq!(config.timeout_seconds, 60);
        assert!(!config.blocked_paths.is_empty());

        // Absolute paths must still be present.
        assert!(config.blocked_paths.contains(&"/etc".to_string()));
        assert!(config
            .blocked_paths
            .contains(&"/var/run/docker.sock".to_string()));

        // Tilde paths must have been expanded — no raw "~/" should remain.
        for p in &config.blocked_paths {
            assert!(
                !p.starts_with("~/"),
                "blocked path '{}' still has unexpanded tilde",
                p
            );
        }

        assert!(config.seccomp_profile.is_none());
        assert!(config.apparmor_profile.is_none());
    }

    #[test]
    fn test_config_serialization_roundtrip() {
        let config = SandboxConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: SandboxConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.image, config.image);
        assert_eq!(deserialized.memory_limit, config.memory_limit);
        assert_eq!(deserialized.network_mode, config.network_mode);
        assert_eq!(deserialized.timeout_seconds, config.timeout_seconds);
    }

    #[test]
    fn test_expand_tilde_expands_home() {
        if let Some(home) = dirs::home_dir() {
            let expanded = expand_tilde("~/.ssh");
            assert!(
                expanded.starts_with(home.to_str().unwrap()),
                "expand_tilde should replace ~ with home dir: got '{}'",
                expanded
            );
            assert!(!expanded.starts_with("~/"), "should not keep raw ~/");
        }
    }

    #[test]
    fn test_expand_tilde_leaves_absolute_unchanged() {
        let path = "/etc/shadow";
        assert_eq!(expand_tilde(path), path);
    }

    #[test]
    fn test_expand_tilde_tilde_only() {
        if let Some(home) = dirs::home_dir() {
            let expanded = expand_tilde("~");
            assert_eq!(expanded, home.to_str().unwrap());
        }
    }

    #[test]
    fn test_expand_tilde_no_home_prefix_unchanged() {
        let path = "/home/user/docs";
        assert_eq!(expand_tilde(path), path);
    }
}
