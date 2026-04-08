use crate::tool_registry::{PermissionDecision, Tool, ToolContext, ToolResult};
use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;

pub struct NotebookEditTool;

#[async_trait]
impl Tool for NotebookEditTool {
    fn name(&self) -> &str { "nexibot_notebook_edit" }
    fn description(&self) -> &str {
        "Edit a Jupyter notebook cell. Actions: replace (change cell content), insert (add new cell), delete (remove cell)."
    }
    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "notebook_path": { "type": "string" },
                "action": { "type": "string", "enum": ["replace", "insert", "delete"] },
                "cell_id": { "type": "string", "description": "Cell ID to target (required for replace/delete)" },
                "content": { "type": "string", "description": "New cell content (required for replace/insert)" },
                "cell_type": { "type": "string", "enum": ["code", "markdown"], "description": "Cell type for insert (default: code)" },
                "after_cell_id": { "type": "string", "description": "Insert after this cell (null = append)" }
            },
            "required": ["notebook_path", "action"]
        })
    }
    async fn check_permissions(&self, input: &Value, _ctx: &ToolContext) -> PermissionDecision {
        match input["notebook_path"].as_str() {
            None => PermissionDecision::Deny("notebook_path is required".to_string()),
            Some(p) if !p.ends_with(".ipynb") => {
                PermissionDecision::Deny(format!("{} is not a .ipynb file", p))
            }
            _ => PermissionDecision::Allow,
        }
    }
    async fn call(&self, input: Value, _ctx: ToolContext) -> ToolResult {
        let path = match input["notebook_path"].as_str() {
            Some(p) => PathBuf::from(p),
            None => return ToolResult::err("notebook_path is required"),
        };
        let action = match input["action"].as_str() {
            Some(a) => a,
            None => return ToolResult::err("action is required"),
        };
        match apply_notebook_edit(&path, action, &input).await {
            Ok(msg) => ToolResult::ok(msg),
            Err(e) => ToolResult::err(e),
        }
    }
}

async fn apply_notebook_edit(
    path: &std::path::Path,
    action: &str,
    input: &Value,
) -> Result<String, String> {
    // Read and parse notebook
    let raw = tokio::fs::read_to_string(path).await
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
    let mut nb: Value = serde_json::from_str(&raw)
        .map_err(|e| format!("Invalid notebook JSON: {}", e))?;
    let nbformat = nb["nbformat"].as_u64().unwrap_or(4);
    let cells = nb["cells"].as_array_mut()
        .ok_or("Notebook has no cells array")?;

    match action {
        "replace" => {
            let cell_id = input["cell_id"].as_str()
                .ok_or("cell_id is required for replace")?;
            let content = input["content"].as_str()
                .ok_or("content is required for replace")?;
            let cell = cells.iter_mut()
                .find(|c| c["id"].as_str() == Some(cell_id))
                .ok_or_else(|| format!("Cell '{}' not found", cell_id))?;
            let cell_type = cell["cell_type"].as_str().unwrap_or("code").to_string();
            cell["source"] = Value::String(content.to_string());
            if cell_type == "code" {
                cell["execution_count"] = Value::Null;
                cell["outputs"] = Value::Array(Vec::new());
            }
            let result = serde_json::to_string_pretty(&nb)
                .map_err(|e| format!("Failed to serialize notebook: {}", e))?;
            tokio::fs::write(path, result).await
                .map_err(|e| format!("Failed to write notebook: {}", e))?;
            Ok(format!("Cell '{}' replaced in {}.", cell_id, path.display()))
        }
        "insert" => {
            let content = input["content"].as_str().ok_or("content is required for insert")?;
            let cell_type = input["cell_type"].as_str().unwrap_or("code");
            let after_id = input["after_cell_id"].as_str();

            let new_cell = make_cell(cell_type, content, nbformat >= 5);
            let new_id = new_cell["id"].as_str().unwrap_or("<auto>").to_string();

            let insert_idx = if let Some(after) = after_id {
                match cells.iter().position(|c| c["id"].as_str() == Some(after)) {
                    Some(i) => i + 1,
                    None => return Err(format!("after_cell_id '{}' not found", after)),
                }
            } else {
                cells.len()
            };

            cells.insert(insert_idx, new_cell);
            let result = serde_json::to_string_pretty(&nb)
                .map_err(|e| format!("Failed to serialize notebook: {}", e))?;
            tokio::fs::write(path, result).await
                .map_err(|e| format!("Failed to write notebook: {}", e))?;
            Ok(format!("New {} cell '{}' inserted in {}.", cell_type, new_id, path.display()))
        }
        "delete" => {
            let cell_id = input["cell_id"].as_str().ok_or("cell_id is required for delete")?;
            let orig_len = cells.len();
            cells.retain(|c| c["id"].as_str() != Some(cell_id));
            if cells.len() == orig_len {
                return Err(format!("Cell '{}' not found", cell_id));
            }
            let result = serde_json::to_string_pretty(&nb)
                .map_err(|e| format!("Failed to serialize notebook: {}", e))?;
            tokio::fs::write(path, result).await
                .map_err(|e| format!("Failed to write notebook: {}", e))?;
            Ok(format!("Cell '{}' deleted from {}.", cell_id, path.display()))
        }
        _ => Err(format!("Unknown action: {}", action)),
    }
}

fn make_cell(cell_type: &str, content: &str, use_id: bool) -> Value {
    if cell_type == "markdown" {
        if use_id {
            let id = uuid::Uuid::new_v4().to_string()[..8].to_string();
            serde_json::json!({
                "cell_type": "markdown",
                "id": id,
                "metadata": {},
                "source": content
            })
        } else {
            serde_json::json!({
                "cell_type": "markdown",
                "metadata": {},
                "source": content
            })
        }
    } else if use_id {
        let id = uuid::Uuid::new_v4().to_string()[..8].to_string();
        serde_json::json!({
            "cell_type": "code",
            "id": id,
            "execution_count": null,
            "metadata": {},
            "outputs": [],
            "source": content
        })
    } else {
        serde_json::json!({
            "cell_type": "code",
            "execution_count": null,
            "metadata": {},
            "outputs": [],
            "source": content
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_notebook(cells: Vec<Value>) -> Value {
        serde_json::json!({
            "nbformat": 4,
            "nbformat_minor": 5,
            "metadata": {},
            "cells": cells
        })
    }

    async fn write_notebook(path: &std::path::Path, nb: &Value) {
        tokio::fs::write(path, serde_json::to_string_pretty(nb).unwrap()).await.unwrap();
    }

    #[tokio::test]
    async fn test_replace_cell_content() {
        let tmp = tempfile::Builder::new().suffix(".ipynb").tempfile().unwrap();
        let nb = make_notebook(vec![serde_json::json!({
            "cell_type": "code", "id": "abc1", "execution_count": 3,
            "metadata": {}, "outputs": [{"output": "old"}],
            "source": "x = 1"
        })]);
        write_notebook(tmp.path(), &nb).await;

        let result = apply_notebook_edit(
            tmp.path(), "replace",
            &serde_json::json!({"cell_id": "abc1", "content": "x = 42"})
        ).await;
        assert!(result.is_ok());

        let raw = tokio::fs::read_to_string(tmp.path()).await.unwrap();
        let updated: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(updated["cells"][0]["source"], "x = 42");
        assert_eq!(updated["cells"][0]["execution_count"], Value::Null);
        assert_eq!(updated["cells"][0]["outputs"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_insert_cell_appends_at_end() {
        let tmp = tempfile::Builder::new().suffix(".ipynb").tempfile().unwrap();
        let nb = make_notebook(vec![serde_json::json!({
            "cell_type": "code", "id": "cell1",
            "execution_count": null, "metadata": {}, "outputs": [],
            "source": "import pandas"
        })]);
        write_notebook(tmp.path(), &nb).await;

        let result = apply_notebook_edit(
            tmp.path(), "insert",
            &serde_json::json!({"content": "df = pd.read_csv('data.csv')", "cell_type": "code"})
        ).await;
        assert!(result.is_ok());

        let raw = tokio::fs::read_to_string(tmp.path()).await.unwrap();
        let updated: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(updated["cells"].as_array().unwrap().len(), 2);
        assert_eq!(updated["cells"][1]["source"], "df = pd.read_csv('data.csv')");
    }

    #[tokio::test]
    async fn test_delete_cell_removes_it() {
        let tmp = tempfile::Builder::new().suffix(".ipynb").tempfile().unwrap();
        let nb = make_notebook(vec![
            serde_json::json!({"cell_type": "code", "id": "keep1", "execution_count": null, "metadata": {}, "outputs": [], "source": "x=1"}),
            serde_json::json!({"cell_type": "code", "id": "del2", "execution_count": null, "metadata": {}, "outputs": [], "source": "y=2"}),
        ]);
        write_notebook(tmp.path(), &nb).await;

        let result = apply_notebook_edit(
            tmp.path(), "delete",
            &serde_json::json!({"cell_id": "del2"})
        ).await;
        assert!(result.is_ok());

        let raw = tokio::fs::read_to_string(tmp.path()).await.unwrap();
        let updated: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(updated["cells"].as_array().unwrap().len(), 1);
        assert_eq!(updated["cells"][0]["id"], "keep1");
    }

    #[tokio::test]
    async fn test_delete_nonexistent_cell_errors() {
        let tmp = tempfile::Builder::new().suffix(".ipynb").tempfile().unwrap();
        let nb = make_notebook(vec![
            serde_json::json!({"cell_type": "code", "id": "only1", "execution_count": null, "metadata": {}, "outputs": [], "source": "x=1"}),
        ]);
        write_notebook(tmp.path(), &nb).await;
        let result = apply_notebook_edit(
            tmp.path(), "delete",
            &serde_json::json!({"cell_id": "doesnotexist"})
        ).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_permission_rejects_non_ipynb() {
        let tool = NotebookEditTool;
        let ctx = ToolContext {
            session_key: "s".into(), agent_id: "a".into(),
            working_dir: PathBuf::from("/tmp"),
        };
        let input = serde_json::json!({"notebook_path": "/tmp/file.py", "action": "replace"});
        let decision = tool.check_permissions(&input, &ctx).await;
        assert!(matches!(decision, PermissionDecision::Deny(_)));
    }

    #[tokio::test]
    async fn test_replace_markdown_cell_does_not_clear_outputs() {
        let tmp = tempfile::Builder::new().suffix(".ipynb").tempfile().unwrap();
        let nb = make_notebook(vec![serde_json::json!({
            "cell_type": "markdown", "id": "md1",
            "metadata": {}, "source": "# Old heading"
        })]);
        write_notebook(tmp.path(), &nb).await;

        let result = apply_notebook_edit(
            tmp.path(), "replace",
            &serde_json::json!({"cell_id": "md1", "content": "# New heading"})
        ).await;
        assert!(result.is_ok());

        let raw = tokio::fs::read_to_string(tmp.path()).await.unwrap();
        let updated: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(updated["cells"][0]["source"], "# New heading");
        // Markdown cells do not have execution_count or outputs — verify they were not added
        assert!(updated["cells"][0]["execution_count"].is_null() || updated["cells"][0].get("execution_count").is_none());
    }

    #[tokio::test]
    async fn test_insert_cell_after_specified_cell() {
        let tmp = tempfile::Builder::new().suffix(".ipynb").tempfile().unwrap();
        let nb = make_notebook(vec![
            serde_json::json!({"cell_type": "code", "id": "first", "execution_count": null, "metadata": {}, "outputs": [], "source": "a=1"}),
            serde_json::json!({"cell_type": "code", "id": "last", "execution_count": null, "metadata": {}, "outputs": [], "source": "b=2"}),
        ]);
        write_notebook(tmp.path(), &nb).await;

        let result = apply_notebook_edit(
            tmp.path(), "insert",
            &serde_json::json!({"content": "c=3", "cell_type": "code", "after_cell_id": "first"})
        ).await;
        assert!(result.is_ok());

        let raw = tokio::fs::read_to_string(tmp.path()).await.unwrap();
        let updated: Value = serde_json::from_str(&raw).unwrap();
        let cells = updated["cells"].as_array().unwrap();
        assert_eq!(cells.len(), 3);
        assert_eq!(cells[0]["id"], "first");
        assert_eq!(cells[1]["source"], "c=3");
        assert_eq!(cells[2]["id"], "last");
    }
}
