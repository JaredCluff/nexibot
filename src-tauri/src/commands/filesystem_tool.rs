//! Built-in nexibot_filesystem tool — file read/write/list without MCP.

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::info;

use crate::config::FilesystemConfig;
use crate::security::external_content;
use crate::security::workspace::{self, WorkspaceConfig};

/// Get the tool definition to pass to Claude.
pub fn nexibot_filesystem_tool_definition() -> Value {
    json!({
        "name": "nexibot_filesystem",
        "description": "Read, write, and list files on the local filesystem. Supports read_file, write_file, list_directory, create_directory, delete_file, and file_info actions. Paths are validated against allowed/blocked directories for security.",
        "input_schema": {
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["read_file", "write_file", "list_directory", "create_directory", "delete_file", "file_info"],
                    "description": "The filesystem operation to perform"
                },
                "path": {
                    "type": "string",
                    "description": "The file or directory path"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write (required for write_file)"
                }
            },
            "required": ["action", "path"]
        }
    })
}

/// Execute the filesystem tool.
pub fn execute_filesystem_tool(input: &Value, config: &FilesystemConfig) -> String {
    if !config.enabled {
        return "Error: Filesystem tool is disabled in settings. Enable it under filesystem.enabled in config.yaml.".to_string();
    }

    let action = match input.get("action").and_then(|a| a.as_str()) {
        Some(a) => a,
        None => return "Error: 'action' is required".to_string(),
    };

    let path_str = match input.get("path").and_then(|p| p.as_str()) {
        Some(p) if !p.trim().is_empty() => p.trim(),
        _ => return "Error: 'path' is required".to_string(),
    };

    // Expand ~ to home directory
    let expanded_path = if path_str.starts_with('~') {
        match dirs::home_dir() {
            Some(home) => home.join(path_str[1..].trim_start_matches(&['/', '\\'][..])),
            None => PathBuf::from(path_str),
        }
    } else {
        PathBuf::from(path_str)
    };

    // Workspace confinement check.
    // Transfer user-configured allowed_paths into extra_allowed so that paths
    // added in Settings → Filesystem Security are actually honoured here.
    // Without this, WorkspaceConfig::default() always rejects anything outside
    // the hardcoded ~/.config/ai/.../workspace root, making allowed_paths useless.
    let ws_config = {
        let mut cfg = WorkspaceConfig::default();
        cfg.extra_allowed = config
            .allowed_paths
            .iter()
            .map(|s| {
                // Expand ~ so that "~/Documents" works as well as an absolute path.
                if s.starts_with('~') {
                    dirs::home_dir()
                        .map(|h| h.join(s[1..].trim_start_matches(&['/', '\\'][..])))
                        .unwrap_or_else(|| PathBuf::from(s))
                } else {
                    PathBuf::from(s)
                }
            })
            .collect();
        cfg
    };
    if let Err(e) = workspace::validate_workspace_path(&expanded_path, &ws_config) {
        return format!("Error: {}", e);
    }

    // Validate path
    if let Err(e) = validate_path(&expanded_path, config) {
        return format!("Error: {}", e);
    }

    match action {
        "read_file" => read_file(&expanded_path, config),
        "write_file" => {
            let content = match input.get("content").and_then(|c| c.as_str()) {
                Some(c) => c,
                None => return "Error: 'content' is required for write_file".to_string(),
            };
            write_file(&expanded_path, content, config)
        }
        "list_directory" => list_directory(&expanded_path),
        "create_directory" => create_directory(&expanded_path),
        "delete_file" => delete_file(&expanded_path),
        "file_info" => file_info(&expanded_path),
        _ => format!("Error: Unknown action '{}'. Use read_file, write_file, list_directory, create_directory, delete_file, or file_info.", action),
    }
}

/// Validate a path against allowed/blocked path lists.
fn validate_path(path: &Path, config: &FilesystemConfig) -> Result<(), String> {
    // Canonicalize to resolve .. and symlinks (if path exists)
    let canonical = if path.exists() {
        path.canonicalize()
            .map_err(|e| format!("Failed to resolve path: {}", e))?
    } else {
        // For non-existent paths, canonicalize the parent
        if let Some(parent) = path.parent() {
            if parent.exists() {
                let canonical_parent = parent
                    .canonicalize()
                    .map_err(|e| format!("Failed to resolve parent path: {}", e))?;
                let file_name = path.file_name().ok_or_else(|| {
                    "Path has no valid file name component (ends with '..' or '.')".to_string()
                })?;
                canonical_parent.join(file_name)
            } else {
                path.to_path_buf()
            }
        } else {
            path.to_path_buf()
        }
    };

    let canonical_str = canonical.to_string_lossy().to_string();

    // Check blocked paths.
    // Use Path::starts_with() (component-aware) rather than str::starts_with() to
    // avoid false positives like "/etc2" matching a block on "/etc".
    for blocked in &config.blocked_paths {
        let blocked_path = PathBuf::from(blocked);
        if canonical.starts_with(&blocked_path) {
            return Err(format!(
                "Path '{}' is in a blocked directory ({})",
                canonical_str, blocked
            ));
        }
    }

    // Check allowed paths (if configured).
    // Same component-aware check: "/home/user/Documents-evil" must NOT match
    // an allow entry of "/home/user/Documents".
    if !config.allowed_paths.is_empty() {
        let allowed = config.allowed_paths.iter().any(|a| {
            let allowed_path = if a.starts_with('~') {
                dirs::home_dir()
                    .map(|h| h.join(a[1..].trim_start_matches(&['/', '\\'][..])))
                    .unwrap_or_else(|| PathBuf::from(a))
            } else {
                PathBuf::from(a)
            };
            canonical.starts_with(&allowed_path)
        });
        if !allowed {
            return Err(format!(
                "Path '{}' is not in the allowed paths list",
                canonical_str
            ));
        }
    } else {
        // Default: allow home directory
        if let Some(home) = dirs::home_dir() {
            if !canonical.starts_with(&home) {
                return Err(format!("Path '{}' is outside the home directory. Configure allowed_paths to access other locations.", canonical_str));
            }
        }
    }

    Ok(())
}

fn read_file(path: &Path, config: &FilesystemConfig) -> String {
    if !path.exists() {
        return format!("Error: File not found: {}", path.display());
    }

    if !path.is_file() {
        return format!(
            "Error: '{}' is not a file. Use list_directory for directories.",
            path.display()
        );
    }

    // Check file size before reading
    match fs::metadata(path) {
        Ok(meta) => {
            if meta.len() as usize > config.max_read_bytes {
                return format!(
                    "Error: File is {} bytes, exceeding the {} byte read limit. Increase filesystem.max_read_bytes in config.",
                    meta.len(),
                    config.max_read_bytes
                );
            }
        }
        Err(e) => return format!("Error: Cannot read file metadata: {}", e),
    }

    info!("[FILESYSTEM] Reading file: {}", path.display());

    match fs::read_to_string(path) {
        Ok(content) => {
            // Wrap file content with external content boundary markers
            let wrapped = external_content::wrap_external_content(
                &content,
                &format!("File: {}", path.display()),
            );
            json!({
                "path": path.display().to_string(),
                "content": wrapped,
                "size": content.len(),
            })
            .to_string()
        }
        Err(e) => {
            // Try reading as binary
            match fs::read(path) {
                Ok(bytes) => json!({
                    "path": path.display().to_string(),
                    "content": format!("[Binary file, {} bytes]", bytes.len()),
                    "size": bytes.len(),
                    "binary": true,
                })
                .to_string(),
                Err(_) => format!("Error: Failed to read file: {}", e),
            }
        }
    }
}

fn write_file(path: &Path, content: &str, config: &FilesystemConfig) -> String {
    if content.len() > config.max_write_bytes {
        return format!(
            "Error: Content is {} bytes, exceeding the {} byte write limit.",
            content.len(),
            config.max_write_bytes
        );
    }

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            if let Err(e) = fs::create_dir_all(parent) {
                return format!("Error: Failed to create parent directory: {}", e);
            }
        }
    }

    info!(
        "[FILESYSTEM] Writing file: {} ({} bytes)",
        path.display(),
        content.len()
    );

    match fs::write(path, content) {
        Ok(()) => json!({
            "path": path.display().to_string(),
            "bytes_written": content.len(),
            "success": true,
        })
        .to_string(),
        Err(e) => format!("Error: Failed to write file: {}", e),
    }
}

fn list_directory(path: &Path) -> String {
    if !path.exists() {
        return format!("Error: Directory not found: {}", path.display());
    }

    if !path.is_dir() {
        return format!(
            "Error: '{}' is not a directory. Use read_file for files.",
            path.display()
        );
    }

    info!("[FILESYSTEM] Listing directory: {}", path.display());

    let mut entries = Vec::new();
    match fs::read_dir(path) {
        Ok(dir) => {
            for entry in dir.flatten() {
                let file_type = if entry.path().is_dir() {
                    "directory"
                } else if entry.path().is_symlink() {
                    "symlink"
                } else {
                    "file"
                };

                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);

                entries.push(json!({
                    "name": entry.file_name().to_string_lossy().to_string(),
                    "type": file_type,
                    "size": size,
                }));
            }
        }
        Err(e) => return format!("Error: Failed to read directory: {}", e),
    }

    // Sort: directories first, then alphabetical
    entries.sort_by(|a, b| {
        let a_type = a.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let b_type = b.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let a_name = a.get("name").and_then(|n| n.as_str()).unwrap_or("");
        let b_name = b.get("name").and_then(|n| n.as_str()).unwrap_or("");

        if a_type == "directory" && b_type != "directory" {
            std::cmp::Ordering::Less
        } else if a_type != "directory" && b_type == "directory" {
            std::cmp::Ordering::Greater
        } else {
            a_name.to_lowercase().cmp(&b_name.to_lowercase())
        }
    });

    json!({
        "path": path.display().to_string(),
        "entries": entries,
        "total": entries.len(),
    })
    .to_string()
}

fn create_directory(path: &Path) -> String {
    if path.exists() {
        return format!("Directory already exists: {}", path.display());
    }

    info!("[FILESYSTEM] Creating directory: {}", path.display());

    match fs::create_dir_all(path) {
        Ok(()) => json!({
            "path": path.display().to_string(),
            "success": true,
        })
        .to_string(),
        Err(e) => format!("Error: Failed to create directory: {}", e),
    }
}

fn delete_file(path: &Path) -> String {
    if !path.exists() {
        return format!("Error: File not found: {}", path.display());
    }

    // Only allow deleting single files, never directories
    if path.is_dir() {
        return "Error: Cannot delete directories with this tool. Only individual files can be deleted.".to_string();
    }

    info!("[FILESYSTEM] Deleting file: {}", path.display());

    match fs::remove_file(path) {
        Ok(()) => json!({
            "path": path.display().to_string(),
            "deleted": true,
        })
        .to_string(),
        Err(e) => format!("Error: Failed to delete file: {}", e),
    }
}

fn file_info(path: &Path) -> String {
    if !path.exists() {
        return format!("Error: Path not found: {}", path.display());
    }

    info!("[FILESYSTEM] Getting file info: {}", path.display());

    match fs::metadata(path) {
        Ok(meta) => {
            let file_type = if meta.is_dir() {
                "directory"
            } else if meta.is_symlink() {
                "symlink"
            } else {
                "file"
            };

            let modified = meta.modified().ok().and_then(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .map(|d| d.as_secs())
            });

            let readonly = meta.permissions().readonly();

            json!({
                "path": path.display().to_string(),
                "type": file_type,
                "size": meta.len(),
                "readonly": readonly,
                "modified_unix": modified,
            })
            .to_string()
        }
        Err(e) => format!("Error: Failed to get file info: {}", e),
    }
}
