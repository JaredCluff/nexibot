//! Plugin discovery and loading.

use std::path::{Path, PathBuf};
use tracing::{info, warn};

use super::registry::PluginRegistry;

/// Discover and load plugins from a directory.
///
/// Each subdirectory is expected to contain plugin metadata.
/// Currently a placeholder for future dynamic loading support.
pub async fn discover_plugins(plugin_dir: &Path) -> Vec<String> {
    if !plugin_dir.exists() {
        info!(
            "[PLUGIN_LOADER] Plugin directory does not exist: {:?}",
            plugin_dir
        );
        return Vec::new();
    }

    let plugin_dir = plugin_dir.to_path_buf();
    let result = tokio::task::spawn_blocking(move || discover_plugins_blocking(&plugin_dir))
        .await
        .unwrap_or_else(|e| {
            warn!("[PLUGIN_LOADER] Plugin discovery task panicked: {}", e);
            Vec::new()
        });

    info!("[PLUGIN_LOADER] Discovered {} plugins", result.len());
    result
}

fn discover_plugins_blocking(plugin_dir: &PathBuf) -> Vec<String> {
    let mut discovered = Vec::new();

    let entries = match std::fs::read_dir(plugin_dir) {
        Ok(e) => e,
        Err(e) => {
            warn!("[PLUGIN_LOADER] Failed to read plugin directory: {}", e);
            return discovered;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let manifest_path = path.join("plugin.json");
        if !manifest_path.exists() {
            continue;
        }

        match std::fs::read_to_string(&manifest_path) {
            Ok(content) => {
                if let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(name) = manifest.get("name").and_then(|n| n.as_str()) {
                        info!("[PLUGIN_LOADER] Discovered plugin: {} at {:?}", name, path);
                        discovered.push(name.to_string());
                    }
                }
            }
            Err(e) => {
                warn!(
                    "[PLUGIN_LOADER] Failed to read manifest {:?}: {}",
                    manifest_path, e
                );
            }
        }
    }

    discovered
}

/// Initialize a plugin registry with built-in plugins.
///
/// In the future, this will also load dynamically discovered plugins.
pub fn create_default_registry() -> PluginRegistry {
    let registry = PluginRegistry::new();
    info!("[PLUGIN_LOADER] Created default plugin registry");
    registry
}
