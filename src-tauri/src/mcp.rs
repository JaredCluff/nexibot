//! MCP (Model Context Protocol) Client Manager
//!
//! Manages connections to MCP servers, discovers tools, and routes tool calls.
//! Uses the official rmcp SDK for JSON-RPC 2.0 communication over stdio.

use anyhow::{Context, Result};
use rmcp::model::CallToolRequestParams;
use rmcp::transport::TokioChildProcess;
use rmcp::ServiceExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::config::{MCPServerConfig, NexiBotConfig};
use crate::security::external_content;
use crate::tool_search::ToolSearchIndex;

/// Connection status for an MCP server
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
    Error(String),
}

/// A discovered tool from an MCP server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredTool {
    /// Original tool name from the MCP server
    pub name: String,
    /// Prefixed name for Claude API (server__toolname)
    pub prefixed_name: String,
    /// Tool description
    pub description: String,
    /// JSON Schema for tool input
    pub input_schema: serde_json::Value,
    /// Which server this tool belongs to
    pub server_name: String,
}

/// Info about a connected MCP server (for UI display)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MCPServerInfo {
    pub name: String,
    pub command: String,
    pub enabled: bool,
    pub status: ConnectionStatus,
    pub tool_count: usize,
    pub tools: Vec<DiscoveredTool>,
}

/// Type alias for the rmcp service handle
type McpService = rmcp::service::RunningService<rmcp::RoleClient, ()>;

/// A single MCP server connection
struct MCPServerConnection {
    config: MCPServerConfig,
    status: ConnectionStatus,
    tools: Vec<DiscoveredTool>,
    service: Option<McpService>,
}

/// Maximum size of tool output in bytes (1MB)
const MAX_TOOL_OUTPUT_BYTES: usize = 1_048_576;

/// Maximum number of concurrent MCP server connections
const MAX_CONCURRENT_SERVERS: usize = 10;

/// MCP Manager - manages multiple MCP server connections
pub struct MCPManager {
    servers: HashMap<String, MCPServerConnection>,
    config: Arc<RwLock<NexiBotConfig>>,
    /// Maps prefixed tool name → (server_name, original_tool_name)
    tool_routing: HashMap<String, (String, String)>,
    /// Semantic search index for filtering MCP tools by relevance
    tool_search_index: ToolSearchIndex,
}

impl MCPManager {
    /// Create a new MCP Manager
    pub fn new(config: Arc<RwLock<NexiBotConfig>>) -> Self {
        Self {
            servers: HashMap::new(),
            config,
            tool_routing: HashMap::new(),
            tool_search_index: ToolSearchIndex::new(Default::default()),
        }
    }

    /// Initialize all configured MCP servers
    pub async fn initialize(&mut self) -> Result<()> {
        let config = self.config.read().await;

        if !config.mcp.enabled {
            info!("[MCP] MCP integration disabled");
            return Ok(());
        }

        let server_configs: Vec<MCPServerConfig> = config.mcp.servers.clone();
        drop(config);

        info!(
            "[MCP] Initializing {} configured servers",
            server_configs.len()
        );

        for server_config in server_configs {
            if !server_config.enabled {
                info!(
                    "[MCP] Server '{}' is disabled, skipping",
                    server_config.name
                );
                self.servers.insert(
                    server_config.name.clone(),
                    MCPServerConnection {
                        config: server_config,
                        status: ConnectionStatus::Disconnected,
                        tools: Vec::new(),
                        service: None,
                    },
                );
                continue;
            }

            match self.connect_server_internal(&server_config).await {
                Ok((tools, service)) => {
                    info!(
                        "[MCP] Server '{}' connected with {} tools",
                        server_config.name,
                        tools.len()
                    );
                    self.servers.insert(
                        server_config.name.clone(),
                        MCPServerConnection {
                            config: server_config,
                            status: ConnectionStatus::Connected,
                            tools,
                            service: Some(service),
                        },
                    );
                }
                Err(e) => {
                    warn!(
                        "[MCP] Failed to connect server '{}': {}",
                        server_config.name, e
                    );
                    self.servers.insert(
                        server_config.name.clone(),
                        MCPServerConnection {
                            config: server_config,
                            status: ConnectionStatus::Error(e.to_string()),
                            tools: Vec::new(),
                            service: None,
                        },
                    );
                }
            }
        }

        // Rebuild tool routing table and search index
        self.rebuild_tool_routing();
        self.rebuild_tool_index().await;

        Ok(())
    }

    /// Connect to a single MCP server (with spawn timeout)
    async fn connect_server_internal(
        &self,
        config: &MCPServerConfig,
    ) -> Result<(Vec<DiscoveredTool>, McpService)> {
        info!(
            "[MCP] Connecting to server '{}': {} {:?}",
            config.name, config.command, config.args
        );

        // Build the command
        let mut cmd = Command::new(&config.command);
        for arg in &config.args {
            cmd.arg(arg);
        }
        for (key, value) in &config.env {
            // Only allow env var names that consist of ASCII alphanumerics and
            // underscores — this blocks LD_PRELOAD-style attacks where a
            // malicious config injects metacharacters or shell-special names.
            if key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                cmd.env(key, value);
            } else {
                warn!("[MCP] Rejected unsafe env var name for server '{}': {:?}", config.name, key);
            }
        }

        // Spawn the child process transport + initialization with timeout
        let spawn_timeout = std::time::Duration::from_secs(30);
        let service = tokio::time::timeout(spawn_timeout, async {
            let transport = TokioChildProcess::new(cmd)
                .context(format!("Failed to spawn MCP server '{}'", config.name))?;
            let service = ()
                .serve(transport)
                .await
                .context(format!("Failed to initialize MCP server '{}'", config.name))?;
            Ok::<_, anyhow::Error>(service)
        })
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "MCP server '{}' connection timed out after 30s",
                config.name
            )
        })??;

        // Discover all tools — list_all_tools() follows pagination cursors until
        // every page has been fetched, preventing tools from being silently dropped.
        let all_tools = service
            .list_all_tools()
            .await
            .context(format!(
                "Failed to list tools from server '{}'",
                config.name
            ))?;

        let tools: Vec<DiscoveredTool> = all_tools
            .into_iter()
            .map(|tool| {
                let prefixed_name = format!("{}__{}", config.name, tool.name);
                DiscoveredTool {
                    name: tool.name.to_string(),
                    prefixed_name,
                    description: tool.description.clone().unwrap_or_default().to_string(),
                    input_schema: serde_json::to_value(&tool.input_schema).unwrap_or_default(),
                    server_name: config.name.clone(),
                }
            })
            .collect();

        for tool in &tools {
            debug!(
                "[MCP] Discovered tool: {} ({})",
                tool.prefixed_name, tool.description
            );
        }

        Ok((tools, service))
    }

    /// Rebuild the tool routing table from all connected servers
    fn rebuild_tool_routing(&mut self) {
        self.tool_routing.clear();
        for (server_name, conn) in &self.servers {
            for tool in &conn.tools {
                self.tool_routing.insert(
                    tool.prefixed_name.clone(),
                    (server_name.clone(), tool.name.clone()),
                );
            }
        }
        info!(
            "[MCP] Tool routing table: {} tools from {} servers",
            self.tool_routing.len(),
            self.servers.len()
        );
    }

    /// Get all tool definitions formatted for the Claude API tools[] array
    pub fn get_all_tools(&self) -> Vec<serde_json::Value> {
        let mut tools = Vec::new();
        for conn in self.servers.values() {
            if conn.status != ConnectionStatus::Connected {
                continue;
            }
            for tool in &conn.tools {
                tools.push(serde_json::json!({
                    "name": tool.prefixed_name,
                    "description": format!("[{}] {}", tool.server_name, tool.description),
                    "input_schema": tool.input_schema,
                }));
            }
        }
        tools
    }

    /// Call a tool by its prefixed name
    pub async fn call_tool(
        &self,
        prefixed_name: &str,
        arguments: serde_json::Value,
    ) -> Result<String> {
        let (server_name, original_name) = self
            .tool_routing
            .get(prefixed_name)
            .ok_or_else(|| anyhow::anyhow!("Unknown tool: {}", prefixed_name))?;

        let conn = self
            .servers
            .get(server_name)
            .ok_or_else(|| anyhow::anyhow!("Server '{}' not found", server_name))?;

        let service = conn
            .service
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Server '{}' not connected", server_name))?;

        info!(
            "[MCP] Calling tool '{}' on server '{}'",
            original_name, server_name
        );
        debug!("[MCP] Arguments: {}", arguments);

        let result = service
            .call_tool(CallToolRequestParams {
                name: original_name.clone().into(),
                arguments: arguments.as_object().cloned(),
                meta: None,
                task: None,
            })
            .await
            .context(format!(
                "Failed to call tool '{}' on server '{}'",
                original_name, server_name
            ))?;

        // Extract text content from the tool result (with size limit)
        let mut output = String::new();
        let mut truncated = false;
        for content in &result.content {
            match content.raw {
                rmcp::model::RawContent::Text(ref text) => {
                    if output.len() + text.text.len() > MAX_TOOL_OUTPUT_BYTES {
                        // Truncate to fit within limit (safe for multi-byte UTF-8)
                        let remaining = MAX_TOOL_OUTPUT_BYTES.saturating_sub(output.len());
                        if remaining > 0 {
                            // Find the last valid UTF-8 char boundary at or before `remaining`
                            let mut safe_end = remaining.min(text.text.len());
                            while safe_end > 0 && !text.text.is_char_boundary(safe_end) {
                                safe_end -= 1;
                            }
                            if safe_end > 0 {
                                output.push_str(&text.text[..safe_end]);
                            }
                        }
                        truncated = true;
                        break;
                    }
                    if !output.is_empty() {
                        output.push('\n');
                    }
                    output.push_str(&text.text);
                }
                _ => {
                    debug!("[MCP] Non-text content from tool, skipping");
                }
            }
        }
        if truncated {
            output.push_str("\n[Output truncated at 1MB]");
            warn!("[MCP] Tool '{}' output truncated at 1MB", original_name);
        }

        if result.is_error.unwrap_or(false) {
            warn!("[MCP] Tool '{}' returned error: {}", original_name, output);
            return Err(anyhow::anyhow!("Tool error: {}", output));
        }

        info!(
            "[MCP] Tool '{}' returned {} chars",
            original_name,
            output.len()
        );
        Ok(external_content::wrap_external_content(
            &output,
            &format!("mcp:{}", server_name),
        ))
    }

    /// Connect a server by name (from config)
    pub async fn connect_server(&mut self, name: &str) -> Result<()> {
        // Check concurrent server limit
        let connected_count = self
            .servers
            .values()
            .filter(|c| c.status == ConnectionStatus::Connected)
            .count();
        if connected_count >= MAX_CONCURRENT_SERVERS {
            anyhow::bail!(
                "Maximum concurrent MCP servers ({}) reached. Disconnect a server before connecting a new one.",
                MAX_CONCURRENT_SERVERS
            );
        }

        let config = {
            let conn = self
                .servers
                .get(name)
                .ok_or_else(|| anyhow::anyhow!("Server '{}' not configured", name))?;
            conn.config.clone()
        };

        match self.connect_server_internal(&config).await {
            Ok((tools, service)) => {
                if let Some(conn) = self.servers.get_mut(name) {
                    conn.status = ConnectionStatus::Connected;
                    conn.tools = tools;
                    conn.service = Some(service);
                }
                self.rebuild_tool_routing();
                self.rebuild_tool_index().await;
                Ok(())
            }
            Err(e) => {
                if let Some(conn) = self.servers.get_mut(name) {
                    conn.status = ConnectionStatus::Error(e.to_string());
                }
                Err(e)
            }
        }
    }

    /// Disconnect a server by name
    pub async fn disconnect_server(&mut self, name: &str) -> Result<()> {
        let conn = self
            .servers
            .get_mut(name)
            .ok_or_else(|| anyhow::anyhow!("Server '{}' not found", name))?;

        if let Some(service) = conn.service.take() {
            let _ = service.cancel().await;
        }
        conn.status = ConnectionStatus::Disconnected;
        conn.tools.clear();
        self.rebuild_tool_routing();
        self.rebuild_tool_index().await;

        info!("[MCP] Disconnected server '{}'", name);
        Ok(())
    }

    /// Add a new server configuration
    pub async fn add_server(&mut self, server_config: MCPServerConfig) -> Result<()> {
        let name = server_config.name.clone();

        // Reject shell interpreters as MCP server commands to prevent IPC-abuse
        // attacks (e.g. XSS → invoke("add_mcp_server", {command: "/bin/bash", ...})).
        const BLOCKED_MCP_COMMANDS: &[&str] = &[
            "bash", "sh", "zsh", "fish", "csh", "tcsh", "dash", "ksh",
        ];
        let cmd_basename = std::path::Path::new(&server_config.command)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&server_config.command);
        if BLOCKED_MCP_COMMANDS.contains(&cmd_basename) {
            return Err(anyhow::anyhow!(
                "Shell executables ('{}') are not permitted as MCP server commands",
                cmd_basename
            ));
        }

        // Save to config
        {
            let mut config = self.config.write().await;
            // Check for duplicate
            if config.mcp.servers.iter().any(|s| s.name == name) {
                return Err(anyhow::anyhow!("Server '{}' already exists", name));
            }
            config.mcp.servers.push(server_config.clone());
            config.save()?;
        }

        // Add to internal state
        self.servers.insert(
            name.clone(),
            MCPServerConnection {
                config: server_config.clone(),
                status: ConnectionStatus::Disconnected,
                tools: Vec::new(),
                service: None,
            },
        );

        // Auto-connect if enabled
        if server_config.enabled {
            if let Err(e) = self.connect_server(&name).await {
                warn!("[MCP] Auto-connect for server '{}' failed: {}", name, e);
            }
        }

        Ok(())
    }

    /// Remove a server configuration
    pub async fn remove_server(&mut self, name: &str) -> Result<()> {
        // Persist removal first so runtime state is only changed after durability.
        {
            let mut config = self.config.write().await;
            let before_len = config.mcp.servers.len();
            config.mcp.servers.retain(|s| s.name != name);
            if config.mcp.servers.len() == before_len {
                return Err(anyhow::anyhow!("Server '{}' not found", name));
            }
            config.save()?;
        }

        if self.servers.contains_key(name) {
            if let Err(e) = self.disconnect_server(name).await {
                warn!(
                    "[MCP] Failed to disconnect server '{}' during removal: {}",
                    name, e
                );
            }
        }

        self.servers.remove(name);
        self.rebuild_tool_routing();
        self.rebuild_tool_index().await;

        info!("[MCP] Removed server '{}'", name);
        Ok(())
    }

    /// Get status information for all servers
    pub fn get_server_info(&self) -> Vec<MCPServerInfo> {
        self.servers
            .values()
            .map(|conn| MCPServerInfo {
                name: conn.config.name.clone(),
                command: conn.config.command.clone(),
                enabled: conn.config.enabled,
                status: conn.status.clone(),
                tool_count: conn.tools.len(),
                tools: conn.tools.clone(),
            })
            .collect()
    }

    /// Check if any MCP tools are available
    #[allow(dead_code)]
    pub fn has_tools(&self) -> bool {
        !self.tool_routing.is_empty()
    }

    /// Rebuild the semantic search index from all connected MCP tools.
    /// Called after connect/disconnect to keep the index in sync.
    async fn rebuild_tool_index(&mut self) {
        let config = self.config.read().await;
        self.tool_search_index
            .update_config(config.mcp.tool_search.clone());
        drop(config);

        self.tool_search_index.clear();
        let all_tools = self.get_all_tools();
        self.tool_search_index.index_tools(&all_tools);
    }

    /// Get filtered MCP tools based on semantic similarity to the query.
    /// Returns only the most relevant MCP tools, reducing context window usage.
    pub fn get_filtered_tools(&self, query: &str) -> Vec<serde_json::Value> {
        self.tool_search_index.search(query)
    }

    /// Look up a single MCP tool by its prefixed name.
    /// Used for dynamic tool addition when the LLM requests a tool
    /// that wasn't in the initial filtered set.
    pub fn get_tool_by_name(&self, name: &str) -> Option<serde_json::Value> {
        self.tool_search_index.get_tool_by_name(name)
    }

    /// Resolve the MCP server name for a given tool (by prefixed name).
    /// Returns `None` if the tool is not in the routing table.
    pub fn get_server_name_for_tool(&self, prefixed_name: &str) -> Option<String> {
        self.tool_routing
            .get(prefixed_name)
            .map(|(server_name, _)| server_name.clone())
    }
}
