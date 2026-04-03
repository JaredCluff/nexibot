use crate::tool_registry::{PermissionDecision, Tool, ToolContext, ToolResult};
use async_trait::async_trait;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub struct FileReadTool;

#[async_trait]
impl Tool for FileReadTool {
    fn name(&self) -> &str { "nexibot_file_read" }
    fn description(&self) -> &str {
        "Read a file. Supports text (with optional offset/limit for line ranges), images (PNG/JPG/GIF/WEBP returned as base64), and binary detection."
    }
    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Absolute path to the file" },
                "offset": { "type": "integer", "description": "Line number to start from (1-indexed)" },
                "limit": { "type": "integer", "description": "Number of lines to read" }
            },
            "required": ["path"]
        })
    }
    fn is_read_only(&self, _: &Value) -> bool { true }
    fn is_concurrency_safe(&self) -> bool { true }

    async fn check_permissions(&self, input: &Value, _ctx: &ToolContext) -> PermissionDecision {
        let path_str = match input["path"].as_str() {
            Some(p) => p,
            None => return PermissionDecision::Deny("path is required".to_string()),
        };
        let path = PathBuf::from(path_str);
        if is_device_path(&path) {
            return PermissionDecision::Deny(
                format!("Reading {} is not allowed (device/proc file)", path_str)
            );
        }
        PermissionDecision::Allow
    }

    async fn call(&self, input: Value, ctx: ToolContext) -> ToolResult {
        let path_str = match input["path"].as_str() {
            Some(p) => p,
            None => return ToolResult::err("path is required"),
        };

        let path = PathBuf::from(path_str);
        let offset = input["offset"].as_u64().map(|v| v as usize);
        let limit = input["limit"].as_u64().map(|v| v as usize);

        match read_file_smart(&path, offset, limit, &ctx).await {
            Ok(output) => ToolResult::ok(output),
            Err(e) => ToolResult::err(format!("Error reading {}: {}", path_str, e)),
        }
    }
}

fn is_device_path(path: &Path) -> bool {
    let s = path.to_string_lossy();
    s.starts_with("/dev/") || s.starts_with("/proc/") || s.starts_with("/sys/")
}

/// Detects if a file is binary by reading the first 8KB and checking for null bytes.
fn is_binary(data: &[u8]) -> bool {
    let sample = &data[..data.len().min(8192)];
    sample.contains(&0u8)
}

async fn read_file_smart(
    path: &Path,
    offset: Option<usize>,
    limit: Option<usize>,
    ctx: &ToolContext,
) -> anyhow::Result<String> {
    let ext = path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    if matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp") {
        return read_image(path).await;
    }

    let bytes = tokio::fs::read(path).await?;

    if is_binary(&bytes) {
        let kind = detect_binary_type(&bytes);
        return Ok(format!("[Binary file, {} bytes, type: {}]", bytes.len(), kind));
    }

    if ext == "ipynb" {
        return read_notebook_summary(&bytes);
    }

    let content = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();

    let start = offset.map(|o| o.saturating_sub(1)).unwrap_or(0).min(total);
    let end = if let Some(lim) = limit {
        (start + lim).min(total)
    } else {
        total
    };

    // FileReadState recording is handled by the command layer; ctx is available for future use
    let _ = ctx;

    let selected: Vec<String> = lines[start..end]
        .iter()
        .enumerate()
        .map(|(i, line)| format!("{:>6}\t{}", start + i + 1, line))
        .collect();

    let mut result = selected.join("\n");

    if offset.is_some() || limit.is_some() {
        result.push_str(&format!(
            "\n\n[Showing lines {}-{} of {}]",
            start + 1,
            end,
            total
        ));
    }

    Ok(result)
}

async fn read_image(path: &Path) -> anyhow::Result<String> {
    let bytes = tokio::fs::read(path).await?;
    let ext = path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png")
        .to_lowercase();

    let final_bytes = if bytes.len() > 1_048_576 {
        resize_image(&bytes, &ext)?
    } else {
        bytes
    };

    let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &final_bytes);
    let mime = match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "image/png",
    };

    Ok(format!("[IMAGE:{mime};base64,{}]", b64))
}

fn resize_image(bytes: &[u8], ext: &str) -> anyhow::Result<Vec<u8>> {
    use image::imageops::FilterType;
    let img = image::load_from_memory(bytes)?;
    let (w, h) = (img.width(), img.height());

    let scale = 1024.0 / (w.max(h) as f32);
    if scale >= 1.0 {
        return Ok(bytes.to_vec());
    }
    let new_w = (w as f32 * scale) as u32;
    let new_h = (h as f32 * scale) as u32;
    let resized = img.resize(new_w, new_h, FilterType::Lanczos3);

    let mut out = Vec::new();
    let format = match ext {
        "jpg" | "jpeg" => image::ImageFormat::Jpeg,
        "gif" => image::ImageFormat::Gif,
        _ => image::ImageFormat::Png,
    };
    resized.write_to(&mut std::io::Cursor::new(&mut out), format)?;
    Ok(out)
}

fn detect_binary_type(data: &[u8]) -> &'static str {
    if data.starts_with(b"\x7fELF") { return "ELF"; }
    if data.starts_with(b"MZ") { return "PE/DLL"; }
    if data.starts_with(b"\xcf\xfa\xed\xfe") || data.starts_with(b"\xce\xfa\xed\xfe") {
        return "Mach-O";
    }
    if data.starts_with(b"PK") { return "ZIP/JAR"; }
    if data.starts_with(b"%PDF") { return "PDF"; }
    "unknown binary"
}

fn read_notebook_summary(bytes: &[u8]) -> anyhow::Result<String> {
    let nb: serde_json::Value = serde_json::from_slice(bytes)?;
    let cells = nb["cells"].as_array()
        .ok_or_else(|| anyhow::anyhow!("not a valid notebook"))?;

    let mut output = String::new();
    for (i, cell) in cells.iter().enumerate() {
        let cell_type = cell["cell_type"].as_str().unwrap_or("unknown");
        let source = cell["source"].as_array()
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<String>())
            .or_else(|| cell["source"].as_str().map(|s| s.to_string()))
            .unwrap_or_default();
        let cell_id = cell["id"].as_str().unwrap_or("?");
        output.push_str(&format!(
            "--- Cell {} [{}] id={} ---\n{}\n",
            i + 1, cell_type, cell_id, source
        ));
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_binary_detects_null_bytes() {
        let binary_data = b"hello\x00world";
        assert!(is_binary(binary_data));
    }

    #[test]
    fn test_is_binary_rejects_text() {
        let text_data = b"fn main() { println!(\"hello\"); }";
        assert!(!is_binary(text_data));
    }

    #[test]
    fn test_is_device_path() {
        assert!(is_device_path(Path::new("/dev/null")));
        assert!(is_device_path(Path::new("/proc/cpuinfo")));
        assert!(!is_device_path(Path::new("/tmp/file.txt")));
        assert!(!is_device_path(Path::new("/home/user/code.rs")));
    }

    #[test]
    fn test_detect_binary_type_elf() {
        let elf = b"\x7fELF\x00\x00\x00";
        assert_eq!(detect_binary_type(elf), "ELF");
    }

    #[test]
    fn test_detect_binary_type_pdf() {
        let pdf = b"%PDF-1.4";
        assert_eq!(detect_binary_type(pdf), "PDF");
    }

    #[tokio::test]
    async fn test_read_text_file_line_range() {
        use std::io::Write;
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        for i in 1..=10 {
            writeln!(tmp, "line {}", i).unwrap();
        }
        let ctx = ToolContext {
            session_key: "s".into(),
            agent_id: "a".into(),
            working_dir: std::path::PathBuf::from("/tmp"),
        };
        let result = read_file_smart(tmp.path(), Some(3), Some(2), &ctx).await.unwrap();
        assert!(result.contains("line 3"));
        assert!(result.contains("line 4"));
        assert!(!result.contains("line 5"));
    }

    #[tokio::test]
    async fn test_read_binary_file_returns_description() {
        use std::io::Write;
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(b"\x7fELF\x00\x00\x00some binary data").unwrap();
        let ctx = ToolContext {
            session_key: "s".into(),
            agent_id: "a".into(),
            working_dir: std::path::PathBuf::from("/tmp"),
        };
        let result = read_file_smart(tmp.path(), None, None, &ctx).await.unwrap();
        assert!(result.contains("[Binary file"));
        assert!(result.contains("ELF"));
    }

    #[tokio::test]
    async fn test_permission_blocks_dev_path() {
        let tool = FileReadTool;
        let ctx = ToolContext {
            session_key: "s".into(),
            agent_id: "a".into(),
            working_dir: std::path::PathBuf::from("/tmp"),
        };
        let input = serde_json::json!({"path": "/dev/null"});
        let decision = tool.check_permissions(&input, &ctx).await;
        assert!(matches!(decision, PermissionDecision::Deny(_)));
    }
}
