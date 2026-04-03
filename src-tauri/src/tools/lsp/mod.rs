pub mod client;
pub mod formatters;
pub mod server_manager;

use crate::tool_registry::{PermissionDecision, Tool, ToolContext, ToolResult};
use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct LspTool {
    pub manager: Arc<RwLock<server_manager::LspServerManager>>,
}

#[async_trait]
impl Tool for LspTool {
    fn name(&self) -> &str { "nexibot_lsp" }
    fn description(&self) -> &str {
        "Language Server Protocol operations: goToDefinition, findReferences, hover, documentSymbol, workspaceSymbol, goToImplementation, incomingCalls, outgoingCalls."
    }
    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["goToDefinition", "findReferences", "hover", "documentSymbol",
                             "workspaceSymbol", "goToImplementation", "incomingCalls", "outgoingCalls"]
                },
                "file_path": { "type": "string", "description": "Absolute path to the file" },
                "line": { "type": "integer", "description": "Line number (1-based)" },
                "character": { "type": "integer", "description": "Character position (1-based)" }
            },
            "required": ["operation", "file_path"]
        })
    }
    fn is_read_only(&self, _: &Value) -> bool { true }
    fn is_concurrency_safe(&self) -> bool { true }

    async fn check_permissions(&self, input: &Value, _ctx: &ToolContext) -> PermissionDecision {
        let mgr = self.manager.read().await;
        if !mgr.has_servers() {
            return PermissionDecision::Deny(
                "No LSP servers configured. Add servers to config.yaml under lsp.servers.".to_string()
            );
        }
        match input["file_path"].as_str() {
            None => PermissionDecision::Deny("file_path is required".to_string()),
            Some(_) => PermissionDecision::Allow,
        }
    }

    async fn call(&self, input: Value, ctx: ToolContext) -> ToolResult {
        let operation = match input["operation"].as_str() {
            Some(op) => op.to_string(),
            None => return ToolResult::err("operation is required"),
        };
        let file_path = match input["file_path"].as_str() {
            Some(p) => PathBuf::from(p),
            None => return ToolResult::err("file_path is required"),
        };
        let line = input["line"].as_u64().unwrap_or(1).saturating_sub(1);
        let character = input["character"].as_u64().unwrap_or(1).saturating_sub(1);

        let position = serde_json::json!({ "line": line, "character": character });
        let uri = format!("file://{}", file_path.to_string_lossy());
        let text_document = serde_json::json!({ "uri": uri });

        let lsp_method = match operation.as_str() {
            "goToDefinition" => "textDocument/definition",
            "findReferences" => "textDocument/references",
            "hover" => "textDocument/hover",
            "documentSymbol" => "textDocument/documentSymbol",
            "workspaceSymbol" => "workspace/symbol",
            "goToImplementation" => "textDocument/implementation",
            "incomingCalls" | "outgoingCalls" => "textDocument/prepareCallHierarchy",
            _ => return ToolResult::err(format!("Unknown operation: {}", operation)),
        };

        let params = match operation.as_str() {
            "documentSymbol" => serde_json::json!({ "textDocument": text_document }),
            "workspaceSymbol" => serde_json::json!({ "query": "" }),
            "findReferences" => serde_json::json!({
                "textDocument": text_document,
                "position": position,
                "context": { "includeDeclaration": true }
            }),
            _ => serde_json::json!({ "textDocument": text_document, "position": position }),
        };

        let base_dir = ctx.working_dir.to_string_lossy().to_string();
        let mut mgr = self.manager.write().await;
        let result = mgr.request(&file_path, lsp_method, params).await;

        match result {
            Err(e) => ToolResult::err(format!("LSP error: {}", e)),
            Ok(value) => {
                let formatted = match operation.as_str() {
                    "hover" => formatters::format_hover(&value),
                    "documentSymbol" | "workspaceSymbol" => formatters::format_symbols(&value),
                    _ => formatters::format_locations(&operation, &value, &base_dir),
                };
                ToolResult::ok(formatted)
            }
        }
    }
}
