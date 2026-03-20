//! Docker container creation, exec, and cleanup.
//!
//! Uses `docker` CLI commands via `tokio::process::Command` to manage
//! sandbox containers with resource limits, network isolation, and
//! security constraints.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::process::Command;
use tracing::{debug, info, warn};

use super::SandboxConfig;
use crate::security::env_sanitize::{self, SanitizeOptions};

/// Label applied to all NexiBot sandbox containers for identification and cleanup.
const SANDBOX_LABEL: &str = "nexibot-sandbox=true";

/// Result of executing a command inside a sandbox container.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecResult {
    /// Standard output from the command.
    pub stdout: String,
    /// Standard error from the command.
    pub stderr: String,
    /// Exit code of the command (137 typically means OOM-killed).
    pub exit_code: i32,
    /// Whether the command was terminated due to timeout.
    pub timed_out: bool,
}

/// Docker sandbox container manager.
///
/// Manages the lifecycle of a single Docker container used for sandboxed
/// command execution. Each `DockerSandbox` instance owns at most one container.
pub struct DockerSandbox {
    /// The container ID, set after `create_container` succeeds.
    container_id: Option<String>,
    /// Configuration controlling resource limits and security.
    config: SandboxConfig,
    /// When the container was created.
    created_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl DockerSandbox {
    /// Create a new sandbox manager with the given configuration.
    pub fn new(config: SandboxConfig) -> Self {
        Self {
            container_id: None,
            config,
            created_at: None,
        }
    }

    /// Return the container ID if one has been created.
    #[allow(dead_code)]
    pub fn container_id(&self) -> Option<&str> {
        self.container_id.as_deref()
    }

    /// Check whether Docker is available on this system.
    ///
    /// Runs `docker version` and returns true if the exit code is 0.
    #[allow(dead_code)]
    pub async fn is_docker_available() -> bool {
        match Command::new("docker")
            .arg("version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
        {
            Ok(status) => {
                let available = status.success();
                debug!("[SANDBOX] Docker availability check: {}", available);
                available
            }
            Err(e) => {
                debug!("[SANDBOX] Docker not available: {}", e);
                false
            }
        }
    }

    /// Create a new sandbox container with security constraints.
    ///
    /// The container is created but not started. Call `start_container` to begin execution.
    /// Returns the container ID on success.
    pub async fn create_container(&mut self) -> Result<String> {
        // Validate config before creating
        let warnings = super::validate::validate_sandbox_config(&self.config)?;
        for w in &warnings {
            warn!("[SANDBOX] Config warning: {}", w);
        }

        let mut args = vec![
            "create".to_string(),
            // Label for identification and cleanup
            "--label".to_string(),
            SANDBOX_LABEL.to_string(),
            // Memory limit
            "--memory".to_string(),
            self.config.memory_limit.clone(),
            // CPU limit
            "--cpus".to_string(),
            self.config.cpu_limit.to_string(),
            // Network isolation
            "--network".to_string(),
            self.config.network_mode.clone(),
            // Read-only root filesystem (except /tmp)
            "--read-only".to_string(),
            // Writable /tmp with size limit
            "--tmpfs".to_string(),
            "/tmp:size=100m".to_string(),
            // Security: prevent privilege escalation
            "--security-opt".to_string(),
            "no-new-privileges".to_string(),
            // Drop all capabilities by default
            "--cap-drop".to_string(),
            "ALL".to_string(),
            // No PID namespace sharing with host
            "--pids-limit".to_string(),
            "256".to_string(),
        ];

        // Sanitize environment variables: strict mode only passes safe vars into the container
        let sanitize_result = env_sanitize::build_safe_env(&SanitizeOptions { strict_mode: true });
        if !sanitize_result.blocked.is_empty() {
            debug!(
                "[SANDBOX] Blocked {} env vars from container: {:?}",
                sanitize_result.blocked.len(),
                sanitize_result.blocked
            );
        }
        for (key, value) in &sanitize_result.allowed {
            args.push("--env".to_string());
            args.push(format!("{}={}", key, value));
        }

        // Image
        args.push(self.config.image.clone());

        // Keep container running with a sleep command
        args.push("sleep".to_string());
        args.push(format!("{}", self.config.timeout_seconds + 30));

        info!(
            "[SANDBOX] Creating container: image={}, memory={}, cpus={}, network={}",
            self.config.image,
            self.config.memory_limit,
            self.config.cpu_limit,
            self.config.network_mode
        );

        let output = Command::new("docker")
            .args(&args)
            .output()
            .await
            .context("Failed to execute docker create")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("docker create failed: {}", stderr.trim());
        }

        let container_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if container_id.is_empty() {
            anyhow::bail!("docker create returned empty container ID");
        }

        info!(
            "[SANDBOX] Container created: {}",
            &container_id[..12.min(container_id.len())]
        );
        self.container_id = Some(container_id.clone());
        self.created_at = Some(chrono::Utc::now());

        Ok(container_id)
    }

    /// Start the sandbox container.
    pub async fn start_container(&self) -> Result<()> {
        let id = self
            .container_id
            .as_ref()
            .context("No container to start")?;

        info!("[SANDBOX] Starting container: {}", &id[..12.min(id.len())]);

        let output = Command::new("docker")
            .args(["start", id])
            .output()
            .await
            .context("Failed to execute docker start")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("docker start failed: {}", stderr.trim());
        }

        debug!("[SANDBOX] Container started successfully");
        Ok(())
    }

    /// Execute a command inside the running sandbox container.
    ///
    /// The command is run with the specified timeout. If it exceeds the timeout,
    /// the process is killed and `timed_out` is set to true in the result.
    pub async fn exec_in_container(&self, command: &str, timeout: Duration) -> Result<ExecResult> {
        let id = self
            .container_id
            .as_ref()
            .context("No container for exec")?;

        // Validate the command before executing
        super::validate::validate_command(command)?;

        debug!(
            "[SANDBOX] Executing in container {}: {}",
            &id[..12.min(id.len())],
            command
        );

        let child = Command::new("docker")
            .args(["exec", id, "sh", "-c", command])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .context("Failed to spawn docker exec")?;

        // Wait with timeout
        match tokio::time::timeout(timeout, child.wait_with_output()).await {
            Ok(Ok(output)) => {
                let exit_code = output.status.code().unwrap_or(-1);
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();

                debug!(
                    "[SANDBOX] Command finished: exit_code={}, stdout_len={}, stderr_len={}",
                    exit_code,
                    stdout.len(),
                    stderr.len()
                );

                Ok(ExecResult {
                    stdout,
                    stderr,
                    exit_code,
                    timed_out: false,
                })
            }
            Ok(Err(e)) => {
                anyhow::bail!("docker exec failed: {}", e);
            }
            Err(_) => {
                warn!(
                    "[SANDBOX] Command timed out after {:?}, killing process",
                    timeout
                );

                // Attempt to kill the exec process inside the container
                let _ = Command::new("docker")
                    .args(["exec", id, "kill", "-9", "-1"])
                    .output()
                    .await;

                Ok(ExecResult {
                    stdout: String::new(),
                    stderr: format!("Command timed out after {} seconds", timeout.as_secs()),
                    exit_code: 124, // Standard timeout exit code
                    timed_out: true,
                })
            }
        }
    }

    /// Stop the sandbox container gracefully (10s grace period).
    pub async fn stop_container(&self) -> Result<()> {
        let id = self.container_id.as_ref().context("No container to stop")?;

        info!("[SANDBOX] Stopping container: {}", &id[..12.min(id.len())]);

        let output = Command::new("docker")
            .args(["stop", "--time", "10", id])
            .output()
            .await
            .context("Failed to execute docker stop")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("[SANDBOX] docker stop warning: {}", stderr.trim());
        }

        debug!("[SANDBOX] Container stopped");
        Ok(())
    }

    /// Force-remove the sandbox container.
    ///
    /// This also clears the stored container ID, allowing a new container
    /// to be created with this manager.
    pub async fn remove_container(&mut self) -> Result<()> {
        let id = match self.container_id.take() {
            Some(id) => id,
            None => return Ok(()), // Nothing to remove
        };

        info!("[SANDBOX] Removing container: {}", &id[..12.min(id.len())]);

        let output = Command::new("docker")
            .args(["rm", "-f", &id])
            .output()
            .await
            .context("Failed to execute docker rm")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("[SANDBOX] docker rm warning: {}", stderr.trim());
        }

        self.created_at = None;
        debug!("[SANDBOX] Container removed");
        Ok(())
    }

    /// Clean up old NexiBot sandbox containers.
    ///
    /// Finds and removes containers with the "nexibot-sandbox" label that were
    /// created more than 1 hour ago. Returns the number of containers removed.
    #[allow(dead_code)]
    pub async fn cleanup_old_containers() -> Result<usize> {
        info!("[SANDBOX] Cleaning up old sandbox containers...");

        // List containers with our label, showing ID and creation time
        let output = Command::new("docker")
            .args([
                "ps",
                "-a",
                "--filter",
                &format!("label={}", SANDBOX_LABEL),
                "--format",
                "{{.ID}}\t{{.CreatedAt}}",
            ])
            .output()
            .await
            .context("Failed to list sandbox containers")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("docker ps failed: {}", stderr.trim());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let now = chrono::Utc::now();
        let one_hour = chrono::Duration::hours(1);
        let mut removed = 0usize;

        for line in stdout.lines() {
            let parts: Vec<&str> = line.splitn(2, '\t').collect();
            if parts.len() < 2 {
                continue;
            }

            let container_id = parts[0].trim();
            let created_str = parts[1].trim();

            // Parse the creation time; if we can't parse, remove it to be safe
            let should_remove = if let Ok(created) =
                chrono::DateTime::parse_from_str(created_str, "%Y-%m-%d %H:%M:%S %z")
            {
                now.signed_duration_since(created.with_timezone(&chrono::Utc)) > one_hour
            } else {
                // Can't parse creation time — remove to be safe
                warn!(
                    "[SANDBOX] Could not parse creation time for container {}: {}",
                    container_id, created_str
                );
                true
            };

            if should_remove {
                debug!("[SANDBOX] Removing old container: {}", container_id);
                let rm_output = Command::new("docker")
                    .args(["rm", "-f", container_id])
                    .output()
                    .await;

                match rm_output {
                    Ok(o) if o.status.success() => {
                        removed += 1;
                    }
                    Ok(o) => {
                        let stderr = String::from_utf8_lossy(&o.stderr);
                        warn!(
                            "[SANDBOX] Failed to remove container {}: {}",
                            container_id,
                            stderr.trim()
                        );
                    }
                    Err(e) => {
                        warn!(
                            "[SANDBOX] Failed to execute docker rm for {}: {}",
                            container_id, e
                        );
                    }
                }
            }
        }

        if removed > 0 {
            info!("[SANDBOX] Cleaned up {} old sandbox containers", removed);
        } else {
            debug!("[SANDBOX] No old sandbox containers to clean up");
        }

        Ok(removed)
    }
}

impl Drop for DockerSandbox {
    fn drop(&mut self) {
        if let Some(ref id) = self.container_id {
            warn!(
                "[SANDBOX] DockerSandbox dropped with active container {}. \
                 Use remove_container() for clean shutdown.",
                &id[..12.min(id.len())]
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exec_result_default_values() {
        let result = ExecResult {
            stdout: "hello".to_string(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        };
        assert_eq!(result.exit_code, 0);
        assert!(!result.timed_out);
        assert_eq!(result.stdout, "hello");
    }

    #[test]
    fn test_exec_result_serialization() {
        let result = ExecResult {
            stdout: "output".to_string(),
            stderr: "error".to_string(),
            exit_code: 1,
            timed_out: true,
        };
        let json = serde_json::to_string(&result).unwrap();
        let deserialized: ExecResult = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.stdout, "output");
        assert_eq!(deserialized.stderr, "error");
        assert_eq!(deserialized.exit_code, 1);
        assert!(deserialized.timed_out);
    }

    #[test]
    fn test_docker_sandbox_new() {
        let config = SandboxConfig::default();
        let sandbox = DockerSandbox::new(config);
        assert!(sandbox.container_id().is_none());
        assert!(sandbox.created_at.is_none());
    }

    #[test]
    fn test_sandbox_label_format() {
        assert!(SANDBOX_LABEL.contains("nexibot-sandbox"));
        assert!(SANDBOX_LABEL.contains("=true"));
    }
}
