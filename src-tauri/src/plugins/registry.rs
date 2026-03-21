//! Plugin registry for managing loaded plugins.

use std::collections::HashMap;
use std::sync::Arc;
use tracing::{info, warn};

use super::trait_def::{
    HookHandler, HookPoint, NexiBotPlugin, PluginCapability, ProviderPlugin, ToolPlugin,
};

/// Metadata about a loaded plugin.
#[derive(Debug, Clone)]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub capability_count: usize,
}

/// Registry of loaded plugins and their capabilities.
pub struct PluginRegistry {
    plugins: Vec<Box<dyn NexiBotPlugin>>,
    plugin_capability_counts: HashMap<String, usize>,
    tools: HashMap<String, Arc<dyn ToolPlugin>>,
    providers: HashMap<String, Arc<dyn ProviderPlugin>>,
    hooks: HashMap<HookPoint, Vec<Arc<dyn HookHandler>>>,
}

impl PluginRegistry {
    /// Create a new empty plugin registry.
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
            plugin_capability_counts: HashMap::new(),
            tools: HashMap::new(),
            providers: HashMap::new(),
            hooks: HashMap::new(),
        }
    }

    /// Register a plugin and extract its capabilities.
    pub fn register(&mut self, plugin: Box<dyn NexiBotPlugin>) {
        let id = plugin.id().to_string();
        let name = plugin.name().to_string();
        let capabilities = plugin.capabilities();
        let capability_count = capabilities.len();

        for capability in capabilities {
            match capability {
                PluginCapability::Tool(tool) => {
                    let tool_name = tool.tool_name().to_string();
                    info!(
                        "[PLUGINS] Registered tool '{}' from plugin '{}'",
                        tool_name, id
                    );
                    self.tools.insert(tool_name, Arc::from(tool));
                }
                PluginCapability::Provider(provider) => {
                    let provider_name = provider.provider_name().to_string();
                    info!(
                        "[PLUGINS] Registered provider '{}' from plugin '{}'",
                        provider_name, id
                    );
                    self.providers.insert(provider_name, Arc::from(provider));
                }
                PluginCapability::Hook(point, handler) => {
                    info!("[PLUGINS] Registered {:?} hook from plugin '{}'", point, id);
                    self.hooks
                        .entry(point)
                        .or_default()
                        .push(Arc::from(handler));
                }
                PluginCapability::Channel(_channel) => {
                    info!("[PLUGINS] Registered channel from plugin '{}'", id);
                    // Channel management is handled separately
                }
            }
        }

        self.plugin_capability_counts.insert(id.clone(), capability_count);

        info!(
            "[PLUGINS] Plugin loaded: {} v{} ({})",
            name,
            plugin.version(),
            id
        );
        self.plugins.push(plugin);
    }

    /// Look up a tool by name.
    pub fn get_tool(&self, name: &str) -> Option<Arc<dyn ToolPlugin>> {
        self.tools.get(name).cloned()
    }

    /// Look up a provider by name.
    pub fn get_provider(&self, name: &str) -> Option<Arc<dyn ProviderPlugin>> {
        self.providers.get(name).cloned()
    }

    /// Get all hooks for a given hook point.
    pub fn get_hooks(&self, point: &HookPoint) -> Vec<Arc<dyn HookHandler>> {
        self.hooks.get(point).cloned().unwrap_or_default()
    }

    /// List all registered tool names.
    pub fn tool_names(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    /// List all registered provider names.
    pub fn provider_names(&self) -> Vec<String> {
        self.providers.keys().cloned().collect()
    }

    /// Get info about all loaded plugins.
    pub fn list_plugins(&self) -> Vec<PluginInfo> {
        self.plugins
            .iter()
            .map(|p| {
                let id = p.id().to_string();
                PluginInfo {
                    id: id.clone(),
                    name: p.name().to_string(),
                    version: p.version().to_string(),
                    capability_count: self.plugin_capability_counts.get(&id).copied().unwrap_or(0),
                }
            })
            .collect()
    }

    /// Check if any plugins are loaded.
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// Shut down all plugins gracefully.
    pub async fn shutdown_all(&self) {
        for plugin in &self.plugins {
            if let Err(e) = plugin.shutdown().await {
                warn!(
                    "[PLUGINS] Error shutting down plugin '{}': {}",
                    plugin.id(),
                    e
                );
            }
        }
        info!("[PLUGINS] All plugins shut down");
    }
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::trait_def::*;
    use async_trait::async_trait;
    use serde_json::Value;

    struct TestPlugin {
        id: String,
    }

    #[async_trait]
    impl NexiBotPlugin for TestPlugin {
        fn id(&self) -> &str { &self.id }
        fn name(&self) -> &str { "Test Plugin" }
        fn version(&self) -> &str { "1.0.0" }
        fn capabilities(&self) -> Vec<PluginCapability> {
            // Return empty since capabilities are consumed during register
            Vec::new()
        }
        async fn initialize(&mut self, _config: &PluginConfig) -> anyhow::Result<()> { Ok(()) }
        async fn shutdown(&self) -> anyhow::Result<()> { Ok(()) }
    }

    #[test]
    fn test_registry_new_is_empty() {
        let registry = PluginRegistry::new();
        assert!(registry.is_empty());
        assert!(registry.tool_names().is_empty());
        assert!(registry.provider_names().is_empty());
    }

    #[test]
    fn test_registry_default() {
        let registry = PluginRegistry::default();
        assert!(registry.is_empty());
    }

    #[test]
    fn test_register_plugin() {
        let mut registry = PluginRegistry::new();
        let plugin = Box::new(TestPlugin {
            id: "test-1".to_string(),
        });
        registry.register(plugin);
        assert!(!registry.is_empty());
        let plugins = registry.list_plugins();
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].id, "test-1");
        assert_eq!(plugins[0].name, "Test Plugin");
        assert_eq!(plugins[0].version, "1.0.0");
    }

    #[test]
    fn test_get_nonexistent_tool() {
        let registry = PluginRegistry::new();
        assert!(registry.get_tool("nonexistent").is_none());
    }

    #[test]
    fn test_get_nonexistent_provider() {
        let registry = PluginRegistry::new();
        assert!(registry.get_provider("nonexistent").is_none());
    }

    #[test]
    fn test_get_hooks_empty() {
        let registry = PluginRegistry::new();
        let hooks = registry.get_hooks(&HookPoint::BeforeMessage);
        assert!(hooks.is_empty());
    }
}
