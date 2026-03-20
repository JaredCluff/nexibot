//! Startup / launch-at-login management commands

use serde::Serialize;
use std::path::Path;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_autostart::AutoLaunchManager;
use tracing::warn;

use crate::config::StartupConfig;

use super::AppState;

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn resolve_k2k_binary_path(configured_path: &str) -> String {
    let configured = Path::new(configured_path);

    if configured.exists() {
        return configured_path.to_string();
    }

    #[cfg(not(windows))]
    let fallbacks: &[&str] = &["/usr/local/bin/knowledge-nexus-agent", "/usr/local/bin/kn-agent"];

    #[cfg(windows)]
    let fallbacks: Vec<String> = {
        let mut paths = Vec::new();
        // Check cargo bin directory
        if let Some(home) = dirs::home_dir() {
            paths.push(
                home.join(".cargo")
                    .join("bin")
                    .join("knowledge-nexus-agent.exe")
                    .to_string_lossy()
                    .to_string(),
            );
        }
        // Check Program Files
        if let Ok(pf) = std::env::var("ProgramFiles") {
            paths.push(format!(r"{}\NexiBot\nexibot-agent.exe", pf));
        }
        paths
    };

    for candidate in fallbacks.iter() {
        let candidate_str: &str = candidate.as_ref();
        if candidate_str != configured_path && Path::new(candidate_str).exists() {
            return candidate_str.to_string();
        }
    }

    configured_path.to_string()
}

#[derive(Debug, Serialize)]
pub struct StartupStatus {
    pub nexibot_enabled: bool,
    pub k2k_agent_enabled: bool,
}

/// Get current startup configuration from config file.
#[tauri::command]
pub async fn get_startup_config(state: State<'_, AppState>) -> Result<StartupConfig, String> {
    let config = state.config.read().await;
    Ok(config.startup.clone())
}

/// Enable or disable NexiBot autostart via tauri-plugin-autostart.
#[tauri::command]
pub async fn set_nexibot_autostart(
    app: AppHandle,
    enabled: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let manager = app.state::<AutoLaunchManager>();
    let previous_runtime_enabled = manager
        .is_enabled()
        .map_err(|e| format!("Failed to check autostart state: {}", e))?;
    if enabled {
        manager
            .enable()
            .map_err(|e| format!("Failed to enable autostart: {}", e))?;
    } else {
        manager
            .disable()
            .map_err(|e| format!("Failed to disable autostart: {}", e))?;
    }

    let mut config = state.config.write().await;
    let previous_config = config.clone();
    config.startup.nexibot_at_login = enabled;
    if let Err(e) = config.save() {
        *config = previous_config;
        drop(config);

        let rollback_result = if previous_runtime_enabled {
            manager.enable()
        } else {
            manager.disable()
        };
        if let Err(rollback_err) = rollback_result {
            warn!(
                "[STARTUP] Failed to rollback NexiBot autostart runtime state after save failure: {}",
                rollback_err
            );
        }

        return Err(e.to_string());
    }
    let _ = state.config_changed.send(());
    Ok(())
}

/// Enable or disable K2K System Agent launch at login.
/// - macOS: LaunchAgent plist
/// - Windows: Registry Run key (HKCU\Software\Microsoft\Windows\CurrentVersion\Run)
#[tauri::command]
pub async fn set_k2k_agent_autostart(
    enabled: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let binary_path = {
        let config = state.config.read().await;
        resolve_k2k_binary_path(&config.startup.k2k_agent_binary)
    };

    if enabled && !Path::new(&binary_path).exists() {
        return Err(format!(
            "K2K agent binary not found at '{}'. Install `kn-agent` or update Startup settings with the correct binary path.",
            binary_path
        ));
    }

    #[cfg(target_os = "macos")]
    {
        let home = dirs::home_dir().ok_or("Cannot determine home directory")?;
        let launch_agents_dir = home.join("Library/LaunchAgents");
        let plist_path = launch_agents_dir.join("ai.nexibot.desktop.agent.plist");

        if enabled {
            std::fs::create_dir_all(&launch_agents_dir)
                .map_err(|e| format!("Failed to create LaunchAgents directory: {}", e))?;

            let log_dir = std::env::temp_dir();
            let escaped_binary = escape_xml(&binary_path);
            let escaped_stdout = escape_xml(&log_dir.join("kn-agent.stdout.log").to_string_lossy());
            let escaped_stderr = escape_xml(&log_dir.join("kn-agent.stderr.log").to_string_lossy());
            let plist_content = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>ai.nexibot.desktop.agent</string>
    <key>ProgramArguments</key>
    <array>
        <string>{escaped_binary}</string>
        <string>start</string>
        <string>--foreground</string>
        <string>--no-tray</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <false/>
    <key>StandardOutPath</key>
    <string>{escaped_stdout}</string>
    <key>StandardErrorPath</key>
    <string>{escaped_stderr}</string>
</dict>
</plist>
"#,
            );

            std::fs::write(&plist_path, plist_content)
                .map_err(|e| format!("Failed to write plist: {}", e))?;
        } else {
            if plist_path.exists() {
                if let Ok(output) = std::process::Command::new("id").arg("-u").output() {
                    if let Ok(uid) = String::from_utf8_lossy(&output.stdout)
                        .trim()
                        .parse::<u32>()
                    {
                        let _ = std::process::Command::new("launchctl")
                            .args([
                                "bootout",
                                &format!("gui/{}", uid),
                                &plist_path.to_string_lossy(),
                            ])
                            .output();
                    }
                }

                std::fs::remove_file(&plist_path)
                    .map_err(|e| format!("Failed to remove plist: {}", e))?;
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        const REG_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
        const VALUE_NAME: &str = "NexiBot";

        if enabled {
            let value = format!(r#""{}" start --foreground --no-tray"#, binary_path);
            let status = crate::platform::hidden_command("reg")
                .args([
                    "add",
                    &format!(r"HKCU\{}", REG_KEY),
                    "/v",
                    VALUE_NAME,
                    "/t",
                    "REG_SZ",
                    "/d",
                    &value,
                    "/f",
                ])
                .status()
                .map_err(|e| format!("Failed to add registry key: {}", e))?;

            if !status.success() {
                return Err("Failed to add autostart registry entry".to_string());
            }
        } else {
            // Delete the registry value (ignore error if it doesn't exist)
            let _ = crate::platform::hidden_command("reg")
                .args([
                    "delete",
                    &format!(r"HKCU\{}", REG_KEY),
                    "/v",
                    VALUE_NAME,
                    "/f",
                ])
                .status();
        }
    }

    #[cfg(target_os = "linux")]
    {
        // Linux: use XDG autostart desktop entry
        let autostart_dir = dirs::config_dir()
            .map(|d| d.join("autostart"))
            .ok_or("Cannot determine config directory")?;
        let desktop_path = autostart_dir.join("kn-agent.desktop");

        if enabled {
            std::fs::create_dir_all(&autostart_dir)
                .map_err(|e| format!("Failed to create autostart directory: {}", e))?;

            // Quote the binary path per XDG desktop entry spec (shell-style quoting).
            // Escape backslashes and double quotes so a path like /usr/bin/foo"bar
            // does not break the Exec field or allow shell injection.
            let quoted_binary = format!(
                "\"{}\"",
                binary_path.replace('\\', "\\\\").replace('"', "\\\"")
            );
            let desktop_content = format!(
                "[Desktop Entry]\nType=Application\nName=Knowledge Nexus Agent\nExec={} start --foreground --no-tray\nHidden=false\nNoDisplay=false\nX-GNOME-Autostart-enabled=true\n",
                quoted_binary
            );

            std::fs::write(&desktop_path, desktop_content)
                .map_err(|e| format!("Failed to write desktop entry: {}", e))?;
        } else {
            if desktop_path.exists() {
                std::fs::remove_file(&desktop_path)
                    .map_err(|e| format!("Failed to remove desktop entry: {}", e))?;
            }
        }
    }

    let mut config = state.config.write().await;
    let previous_config = config.clone();
    config.startup.k2k_agent_binary = binary_path;
    config.startup.k2k_agent_at_login = enabled;
    if let Err(e) = config.save() {
        *config = previous_config;
        drop(config);

        #[cfg(target_os = "macos")]
        {
            let home = dirs::home_dir();
            if let Some(home) = home {
                let launch_agents_dir = home.join("Library/LaunchAgents");
                let plist_path = launch_agents_dir.join("ai.nexibot.desktop.agent.plist");
                let rollback_result = if !enabled && plist_path.exists() {
                    // We deleted it but config save failed — can't restore content, just warn
                    Ok(())
                } else if enabled && plist_path.exists() {
                    // We wrote a new plist but config save failed — remove it
                    std::fs::remove_file(&plist_path)
                } else {
                    Ok(())
                };

                if let Err(rollback_err) = rollback_result {
                    warn!(
                        "[STARTUP] Failed to rollback K2K launch agent file after config save failure: {}",
                        rollback_err
                    );
                }
            }
        }

        return Err(e.to_string());
    }
    let _ = state.config_changed.send(());
    Ok(())
}

/// Get live startup status by checking autostart manager and platform-specific entries.
#[tauri::command]
pub async fn get_startup_status(app: AppHandle) -> Result<StartupStatus, String> {
    let manager = app.state::<AutoLaunchManager>();
    let nexibot_enabled = manager
        .is_enabled()
        .map_err(|e| format!("Failed to check autostart: {}", e))?;

    let k2k_agent_enabled = {
        #[cfg(target_os = "macos")]
        {
            dirs::home_dir()
                .map(|h| {
                    h.join("Library/LaunchAgents/ai.nexibot.desktop.agent.plist")
                        .exists()
                })
                .unwrap_or(false)
        }
        #[cfg(target_os = "windows")]
        {
            // Check if registry key exists
            crate::platform::hidden_command("reg")
                .args([
                    "query",
                    r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                    "/v",
                    "NexiBot",
                ])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        }
        #[cfg(target_os = "linux")]
        {
            dirs::config_dir()
                .map(|d| d.join("autostart/kn-agent.desktop").exists())
                .unwrap_or(false)
        }
    };

    Ok(StartupStatus {
        nexibot_enabled,
        k2k_agent_enabled,
    })
}
