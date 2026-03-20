//! Bridge Service Manager
//!
//! Manages the lifecycle of the Node.js bridge service that provides plugin-based
//! provider SDK integration (Anthropic OAuth, OpenAI, etc.).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::platform::hidden_command;

/// Bridge service status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum BridgeStatus {
    NotInstalled,
    Stopped,
    Starting,
    Running,
    Unhealthy,
    Error(String),
}

/// Plugin info from health response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInfo {
    pub name: String,
    pub version: String,
    pub source: String,
}

/// Bridge service health response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeHealth {
    pub status: String,
    pub service: String,
    pub version: String,
    pub timestamp: String,
    pub plugins: Option<Vec<PluginInfo>>,
}

/// Bridge service manager
pub struct BridgeManager {
    bridge_dir: PathBuf,
    bridge_url: String,
    process: Arc<RwLock<Option<Child>>>,
    status: Arc<RwLock<BridgeStatus>>,
}

impl BridgeManager {
    /// Create a new bridge manager
    pub fn new() -> Self {
        let bridge_url = {
            let raw = std::env::var("ANTHROPIC_BRIDGE_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:18790".to_string());
            if let Err(e) = crate::security::ssrf::validate_loopback_url(&raw) {
                tracing::error!("[BRIDGE] {e}; falling back to default loopback URL");
                "http://127.0.0.1:18790".to_string()
            } else {
                raw
            }
        };

        // Bridge directory is relative to the binary location
        let bridge_dir = Self::get_bridge_dir();

        Self {
            bridge_dir,
            bridge_url,
            process: Arc::new(RwLock::new(None)),
            status: Arc::new(RwLock::new(BridgeStatus::NotInstalled)),
        }
    }

    /// Get the bridge directory path
    fn get_bridge_dir() -> PathBuf {
        // In development: relative to workspace root
        // In production: relative to app resources
        let current_exe = std::env::current_exe().ok();

        if let Some(exe_path) = current_exe {
            info!("[BRIDGE] current_exe: {}", exe_path.display());

            // 1. Development: exe is at src-tauri/target/{debug,release}/nexibot-tauri
            //    Bridge is at nexibot/bridge (3 levels up from exe)
            let mut dev_path = exe_path.clone();
            dev_path.pop(); // Remove exe name
            dev_path.pop(); // Remove debug/release
            dev_path.pop(); // Remove target
            dev_path.pop(); // Remove src-tauri
            dev_path.push("bridge");

            if dev_path.exists() {
                info!("[BRIDGE] Found bridge at dev path: {}", dev_path.display());
                return dev_path;
            }

            // 2. macOS .app bundle: exe is at Contents/MacOS/nexibot-tauri
            //    Resources are at Contents/Resources/
            let mut macos_resources = exe_path.clone();
            macos_resources.pop(); // Remove exe name -> Contents/MacOS/
            macos_resources.pop(); // -> Contents/
            macos_resources.push("Resources");
            macos_resources.push("_bridge_bundle");
            macos_resources.push("bridge");

            if macos_resources.exists() {
                info!(
                    "[BRIDGE] Found bridge at macOS resources: {}",
                    macos_resources.display()
                );
                return macos_resources;
            }

            // 3. Also try without _bridge_bundle prefix (in case Tauri flattens)
            let mut resources_flat = exe_path.clone();
            resources_flat.pop();
            resources_flat.pop();
            resources_flat.push("Resources");
            resources_flat.push("bridge");

            if resources_flat.exists() {
                info!(
                    "[BRIDGE] Found bridge at macOS resources (flat): {}",
                    resources_flat.display()
                );
                return resources_flat;
            }

            // 4. Windows NSIS install: exe is at AppData/Local/NexiBot/nexibot-tauri.exe
            //    Bridge is at AppData/Local/NexiBot/_bridge_bundle/bridge/
            let mut windows_nsis = exe_path.clone();
            windows_nsis.pop(); // Remove exe name
            windows_nsis.push("_bridge_bundle");
            windows_nsis.push("bridge");

            info!("[BRIDGE] Checking Windows NSIS path: {}", windows_nsis.display());
            if windows_nsis.exists() {
                info!(
                    "[BRIDGE] Found bridge at Windows NSIS path: {}",
                    windows_nsis.display()
                );
                return windows_nsis;
            }

            // 5. Sibling of exe (legacy check)
            let mut sibling_path = exe_path.clone();
            sibling_path.pop();
            sibling_path.push("bridge");

            if sibling_path.exists() {
                info!(
                    "[BRIDGE] Found bridge as sibling of exe: {}",
                    sibling_path.display()
                );
                return sibling_path;
            }
        }

        // Fallback to current directory
        warn!("[BRIDGE] Could not find bridge in any expected location, falling back to CWD");
        PathBuf::from("bridge")
    }

    /// Get current bridge URL
    #[allow(dead_code)]
    pub fn get_url(&self) -> String {
        self.bridge_url.clone()
    }

    /// Get current status
    pub async fn get_status(&self) -> BridgeStatus {
        self.status.read().await.clone()
    }

    /// Check if Node.js is installed
    fn check_node_installed(&self) -> Result<String> {
        let output = hidden_command("node")
            .arg("--version")
            .output()
            .context("Failed to check Node.js version")?;

        if !output.status.success() {
            anyhow::bail!("Node.js is not installed");
        }

        let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(version)
    }

    /// Check if bridge dependencies are installed
    fn check_dependencies_installed(&self) -> bool {
        let node_modules = self.bridge_dir.join("node_modules");
        node_modules.exists()
    }

    /// Install bridge dependencies
    pub async fn install_dependencies(&self) -> Result<()> {
        // Skip if already installed
        if self.check_dependencies_installed() {
            info!("[BRIDGE] Dependencies already installed, skipping npm install");
            return Ok(());
        }

        info!(
            "[BRIDGE] Installing dependencies in: {}",
            self.bridge_dir.display()
        );

        *self.status.write().await = BridgeStatus::Starting;

        // Check Node.js
        let node_version = self.check_node_installed().context(
            "Node.js is not installed. Please install Node.js 18+ from https://nodejs.org/",
        )?;
        info!("[BRIDGE] Node.js version: {}", node_version);

        // Run npm install (npm is a .cmd on Windows, must run via cmd)
        let mut npm_cmd = hidden_command(if cfg!(windows) { "cmd" } else { "npm" });
        if cfg!(windows) {
            npm_cmd.args(["/C", "npm", "install"]);
        } else {
            npm_cmd.arg("install");
        }
        let output = npm_cmd
            .current_dir(&self.bridge_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .context("Failed to run npm install")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            *self.status.write().await =
                BridgeStatus::Error(format!("npm install failed: {}", stderr));
            anyhow::bail!("npm install failed: {}", stderr);
        }

        info!("[BRIDGE] Dependencies installed successfully");
        Ok(())
    }

    /// Start the bridge service
    pub async fn start(&self) -> Result<()> {
        info!("[BRIDGE] Starting bridge service...");

        // Check if already running
        let current_status = self.status.read().await.clone();
        if current_status == BridgeStatus::Running {
            info!("[BRIDGE] Bridge is already running");
            return Ok(());
        }

        *self.status.write().await = BridgeStatus::Starting;

        // Check dependencies
        if !self.check_dependencies_installed() {
            info!("[BRIDGE] Dependencies not installed, installing...");
            self.install_dependencies().await?;
        }

        // Kill any existing process on the port
        self.kill_existing_process().await?;

        // Start the bridge process
        let server_js = self.bridge_dir.join("server.js");
        if !server_js.exists() {
            *self.status.write().await = BridgeStatus::Error("server.js not found".to_string());
            anyhow::bail!("Bridge server.js not found at: {}", server_js.display());
        }

        // Determine external plugins directory
        let plugins_dir = std::env::var("BRIDGE_PLUGINS_DIR").unwrap_or_else(|_| {
            let mut default_dir = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
            default_dir.push("nexibot");
            default_dir.push("bridge-plugins");
            default_dir.to_string_lossy().to_string()
        });

        let child = hidden_command("node")
            .arg("server.js")
            .env("BRIDGE_PLUGINS_DIR", &plugins_dir)
            .current_dir(&self.bridge_dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("Failed to start bridge service")?;

        let pid = child.id();
        info!("[BRIDGE] Bridge service started with PID: {}", pid);

        *self.process.write().await = Some(child);

        // Wait for bridge to be ready (retry health check a few times)
        for attempt in 1..=5 {
            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
            match self.check_health().await {
                Ok(_) => {
                    *self.status.write().await = BridgeStatus::Running;
                    info!("[BRIDGE] Bridge service is running and healthy");
                    return Ok(());
                }
                Err(e) => {
                    if attempt < 5 {
                        debug!("[BRIDGE] Health check attempt {}/5 failed: {}", attempt, e);
                    } else {
                        *self.status.write().await = BridgeStatus::Unhealthy;
                        warn!("[BRIDGE] Bridge started but health check failed after 5 attempts: {}", e);
                        return Err(e);
                    }
                }
            }
        }
        unreachable!()
    }

    /// Stop the bridge service
    pub async fn stop(&self) -> Result<()> {
        info!("[BRIDGE] Stopping bridge service...");

        let mut process = self.process.write().await;
        if let Some(mut child) = process.take() {
            child.kill().context("Failed to kill bridge process")?;
            child.wait().context("Failed to wait for bridge process")?;
            info!("[BRIDGE] Bridge service stopped");
        }

        *self.status.write().await = BridgeStatus::Stopped;
        Ok(())
    }

    /// Kill existing process on bridge port
    async fn kill_existing_process(&self) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            let output = hidden_command("lsof").args(["-ti:18790"]).output();

            if let Ok(output) = output {
                if output.status.success() {
                    let pids = String::from_utf8_lossy(&output.stdout);
                    for pid in pids.lines() {
                        if let Ok(pid_num) = pid.trim().parse::<u32>() {
                            info!(
                                "[BRIDGE] Killing existing process on port 18790: PID {}",
                                pid_num
                            );
                            let _ = hidden_command("kill")
                                .args(["-9", &pid_num.to_string()])
                                .output();
                        }
                    }
                }
            }
        }

        #[cfg(target_os = "linux")]
        {
            let output = hidden_command("fuser").args(&["-k", "18790/tcp"]).output();

            if let Ok(output) = output {
                if output.status.success() {
                    info!("[BRIDGE] Killed existing process on port 18790");
                }
            }
        }

        #[cfg(target_os = "windows")]
        {
            // Use netstat to find the PID listening on port 18790, then taskkill it
            let output = hidden_command("netstat")
                .args(["-ano"])
                .output();

            if let Ok(output) = output {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    if line.contains(":18790") && line.contains("LISTENING") {
                        // Last column is the PID
                        if let Some(pid) = line.split_whitespace().last() {
                            if let Ok(pid_num) = pid.trim().parse::<u32>() {
                                info!(
                                    "[BRIDGE] Killing existing process on port 18790: PID {}",
                                    pid_num
                                );
                                let _ = hidden_command("taskkill")
                                    .args(["/PID", &pid_num.to_string(), "/F"])
                                    .output();
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Check bridge health
    pub async fn check_health(&self) -> Result<BridgeHealth> {
        let client = reqwest::Client::new();
        let health_url = format!("{}/health", self.bridge_url);

        let response = client
            .get(&health_url)
            .timeout(tokio::time::Duration::from_secs(5))
            .send()
            .await
            .context("Failed to check bridge health")?;

        if !response.status().is_success() {
            anyhow::bail!(
                "Bridge health check failed with status: {}",
                response.status()
            );
        }

        let health: BridgeHealth = response
            .json()
            .await
            .context("Failed to parse bridge health response")?;

        debug!("[BRIDGE] Health check: {:?}", health);
        Ok(health)
    }

    /// Restart the bridge service
    pub async fn restart(&self) -> Result<()> {
        info!("[BRIDGE] Restarting bridge service...");
        self.stop().await?;
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        self.start().await?;
        Ok(())
    }

    /// Ensure bridge is running (start if not)
    pub async fn ensure_running(&self) -> Result<()> {
        let mut retries = 0;
        loop {
            if retries > 3 {
                return Err(anyhow::anyhow!("Bridge failed to start after 3 retries"));
            }

            let status = self.get_status().await;

            match status {
                BridgeStatus::Running => {
                    // Verify with health check
                    match self.check_health().await {
                        Ok(_) => {
                            debug!("[BRIDGE] Bridge is running and healthy");
                            return Ok(());
                        }
                        Err(_) => {
                            warn!("[BRIDGE] Bridge is running but unhealthy, restarting...");
                            return self.restart().await;
                        }
                    }
                }
                BridgeStatus::NotInstalled => {
                    info!("[BRIDGE] Bridge not installed, setting up...");
                    self.install_dependencies().await?;
                    return self.start().await;
                }
                BridgeStatus::Stopped | BridgeStatus::Unhealthy => {
                    info!("[BRIDGE] Bridge is stopped, starting...");
                    return self.start().await;
                }
                BridgeStatus::Starting => {
                    // Wait for it to finish starting
                    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                    retries += 1;
                }
                BridgeStatus::Error(e) => {
                    error!("[BRIDGE] Bridge is in error state: {}", e);
                    info!("[BRIDGE] Attempting to restart...");
                    return self.restart().await;
                }
            }
        }
    }
}

impl Drop for BridgeManager {
    fn drop(&mut self) {
        // Try to stop the bridge when manager is dropped
        if let Ok(mut process) = self.process.try_write() {
            if let Some(mut child) = process.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}
