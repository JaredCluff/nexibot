//! Core plugin traits and capability types.

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Configuration passed to plugins during initialization.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginConfig {
    /// Plugin-specific configuration values.
    #[serde(default)]
    pub settings: std::collections::HashMap<String, Value>,
}

/// Points where hooks can intercept the processing pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HookPoint {
    /// Before a user message is processed.
    BeforeMessage,
    /// After a response is generated.
    AfterMessage,
    /// Before a tool call is executed.
    BeforeToolCall,
    /// After a tool call completes.
    AfterToolCall,
    /// Before an LLM API call.
    BeforeModelCall,
    /// After an LLM API call returns.
    AfterModelCall,
    /// On error during processing.
    OnError,
}

/// Core trait for NexiBot plugins.
#[async_trait]
pub trait NexiBotPlugin: Send + Sync {
    /// Unique plugin identifier.
    fn id(&self) -> &str;
    /// Human-readable plugin name.
    fn name(&self) -> &str;
    /// Plugin version (semver).
    fn version(&self) -> &str;
    /// Capabilities provided by this plugin.
    fn capabilities(&self) -> Vec<PluginCapability>;
    /// Initialize the plugin with configuration.
    async fn initialize(&mut self, config: &PluginConfig) -> Result<()>;
    /// Gracefully shut down the plugin.
    async fn shutdown(&self) -> Result<()>;
}

/// Capability that a plugin provides.
pub enum PluginCapability {
    /// An LLM provider (e.g., Google Gemini, DeepSeek).
    Provider(Box<dyn ProviderPlugin>),
    /// A tool that the LLM can invoke.
    Tool(Box<dyn ToolPlugin>),
    /// A messaging channel (e.g., a new chat platform).
    Channel(Box<dyn ChannelPlugin>),
    /// A hook handler for a specific processing point.
    Hook(HookPoint, Box<dyn HookHandler>),
}

/// Trait for provider plugins.
#[async_trait]
pub trait ProviderPlugin: Send + Sync {
    /// Provider name.
    fn provider_name(&self) -> &str;
    /// List of supported model IDs.
    fn supported_models(&self) -> Vec<String>;
    /// Send a message and get a response.
    async fn send_message(&self, messages: &[Value], model: &str) -> Result<Value>;
    /// Send a message with streaming response.
    async fn send_message_stream(
        &self,
        messages: &[Value],
        model: &str,
        sender: tokio::sync::mpsc::Sender<String>,
    ) -> Result<()>;
}

/// Trait for tool plugins.
#[async_trait]
pub trait ToolPlugin: Send + Sync {
    /// Tool name (used in tool_use blocks).
    fn tool_name(&self) -> &str;
    /// Tool description for the LLM.
    fn description(&self) -> &str;
    /// JSON Schema for the tool's input.
    fn input_schema(&self) -> Value;
    /// Execute the tool with the given input.
    async fn execute(&self, input: &Value) -> Result<Value>;
}

/// Trait for channel plugins.
#[async_trait]
pub trait ChannelPlugin: Send + Sync {
    /// Channel name.
    fn channel_name(&self) -> &str;
    /// Start listening for inbound messages.
    async fn start(&self) -> Result<()>;
    /// Stop listening.
    async fn stop(&self) -> Result<()>;
    /// Send an outbound message.
    async fn send(&self, target: &str, content: &str) -> Result<()>;
}

/// Trait for hook handlers.
#[async_trait]
pub trait HookHandler: Send + Sync {
    /// Process a hook event. Return modified data or None to cancel.
    async fn handle(&self, data: Value) -> Result<Option<Value>>;
}
