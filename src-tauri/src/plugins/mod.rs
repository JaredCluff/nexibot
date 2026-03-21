//! Plugin system for extending NexiBot with custom providers, tools, channels, and hooks.
//!
//! Supports both bridge (Node.js) plugins and native Rust plugins.
//! Native plugins implement the `NexiBotPlugin` trait and are discovered
//! at startup from the configured plugin directories.

pub mod loader;
pub mod registry;
pub mod trait_def;

pub use registry::PluginRegistry;
pub use trait_def::{
    ChannelPlugin, HookHandler, HookPoint, NexiBotPlugin, PluginCapability, PluginConfig,
    ProviderPlugin, ToolPlugin,
};
