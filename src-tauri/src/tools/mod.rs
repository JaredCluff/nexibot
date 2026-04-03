//! New trait-based tools for NexiBot v0.9.0.
//! Each submodule implements crate::tool_registry::Tool.

pub mod file_read;
pub mod file_read_state;

/// Register all v0.9.0 tools into the registry.
/// Called once at startup from AppState initialization.
pub fn register_all(registry: &mut crate::tool_registry::ToolRegistry) {
    registry.register(Box::new(file_read::FileReadTool));
}
