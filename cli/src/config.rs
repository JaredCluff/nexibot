//! CLI configuration loading and management

use crate::error::CliError;
use dirs::config_dir;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize)]
pub struct CliConfig {
    /// API server URL
    pub api_url: Option<String>,

    /// API authentication token
    pub token: Option<String>,

    /// Default output format
    pub default_format: Option<String>,

    /// Default timeout in seconds
    pub timeout: Option<u64>,

    /// Aliases for commands
    #[serde(default)]
    pub aliases: std::collections::HashMap<String, String>,
}

impl Default for CliConfig {
    fn default() -> Self {
        Self {
            api_url: Some("http://localhost:18791".to_string()),
            token: None,
            default_format: Some("table".to_string()),
            timeout: Some(30),
            aliases: std::collections::HashMap::new(),
        }
    }
}

impl CliConfig {
    /// Load configuration from file or use defaults
    pub fn load(config_path: &Option<PathBuf>) -> Result<Self, CliError> {
        let path = if let Some(p) = config_path {
            p.clone()
        } else {
            let mut default_path = config_dir().ok_or_else(|| {
                CliError::Config("Could not determine config directory".to_string())
            })?;
            default_path.push("nexibot/cli.toml");
            default_path
        };

        if !path.exists() {
            return Ok(CliConfig::default());
        }

        let content = fs::read_to_string(&path)
            .map_err(|e| CliError::Config(format!("Failed to read config: {}", e)))?;

        toml::from_str(&content)
            .map_err(|e| CliError::Config(format!("Failed to parse config: {}", e)))
    }

    /// Save configuration to file
    pub fn save(&self, config_path: &Option<PathBuf>) -> Result<(), CliError> {
        let path = if let Some(p) = config_path {
            p.clone()
        } else {
            let mut default_path = config_dir().ok_or_else(|| {
                CliError::Config("Could not determine config directory".to_string())
            })?;
            default_path.push("nexibot/cli.toml");
            default_path
        };

        // Create parent directories if needed
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                CliError::Config(format!("Failed to create config directory: {}", e))
            })?;
        }

        let content = toml::to_string_pretty(self)
            .map_err(|e| CliError::Config(format!("Failed to serialize config: {}", e)))?;

        fs::write(&path, content)
            .map_err(|e| CliError::Config(format!("Failed to write config: {}", e)))?;

        // Restrict to owner-only: config contains auth tokens
        restrict_config_permissions(&path)
            .map_err(|e| CliError::Config(format!("Failed to set config permissions: {}", e)))
    }
}

/// Set config file permissions to owner read/write only (0o600 on Unix).
fn restrict_config_permissions(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(windows)]
    {
        let username = std::env::var("USERNAME").unwrap_or_default();
        if !username.is_empty() {
            let path_str = path.to_string_lossy();
            let _ = std::process::Command::new("icacls")
                .args([&*path_str, "/inheritance:r"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            let grant = format!("{}:(R,W)", username);
            let _ = std::process::Command::new("icacls")
                .args([&*path_str, "/grant:r", &grant])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }
    Ok(())
}
