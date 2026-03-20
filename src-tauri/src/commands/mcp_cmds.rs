//! MCP (Model Context Protocol) management commands

use tauri::State;
use tracing::info;

use crate::config::MCPServerConfig;
use crate::mcp::MCPServerInfo;

use super::AppState;

/// List all configured MCP servers with status and tool info
#[tauri::command]
pub async fn list_mcp_servers(state: State<'_, AppState>) -> Result<Vec<MCPServerInfo>, String> {
    let mcp = state.mcp_manager.read().await;
    Ok(mcp.get_server_info())
}

/// List all discovered tools across all connected MCP servers
#[tauri::command]
pub async fn list_mcp_tools(state: State<'_, AppState>) -> Result<Vec<serde_json::Value>, String> {
    let mcp = state.mcp_manager.read().await;
    Ok(mcp.get_all_tools())
}

/// Connect to a configured MCP server by name
#[tauri::command]
pub async fn connect_mcp_server(state: State<'_, AppState>, name: String) -> Result<(), String> {
    info!("[MCP] Connecting server: {}", name);
    let mut mcp = state.mcp_manager.write().await;
    mcp.connect_server(&name).await.map_err(|e| e.to_string())
}

/// Disconnect an MCP server by name
#[tauri::command]
pub async fn disconnect_mcp_server(state: State<'_, AppState>, name: String) -> Result<(), String> {
    info!("[MCP] Disconnecting server: {}", name);
    let mut mcp = state.mcp_manager.write().await;
    mcp.disconnect_server(&name)
        .await
        .map_err(|e| e.to_string())
}

/// Add a new MCP server configuration
#[tauri::command]
pub async fn add_mcp_server(
    state: State<'_, AppState>,
    config: MCPServerConfig,
) -> Result<(), String> {
    info!("[MCP] Adding server: {}", config.name);
    let mut mcp = state.mcp_manager.write().await;
    mcp.add_server(config).await.map_err(|e| e.to_string())?;
    let _ = state.config_changed.send(());
    Ok(())
}

/// Remove an MCP server configuration
#[tauri::command]
pub async fn remove_mcp_server(state: State<'_, AppState>, name: String) -> Result<(), String> {
    info!("[MCP] Removing server: {}", name);
    let mut mcp = state.mcp_manager.write().await;
    mcp.remove_server(&name).await.map_err(|e| e.to_string())?;
    let _ = state.config_changed.send(());
    Ok(())
}
