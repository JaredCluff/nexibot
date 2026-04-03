use crate::tool_registry::{PermissionDecision, Tool, ToolContext, ToolResult};
use async_trait::async_trait;
use serde_json::Value;
use similar::{ChangeTag, TextDiff};
use std::path::{Path, PathBuf};

pub struct FileEditTool;

#[async_trait]
impl Tool for FileEditTool {
    fn name(&self) -> &str { "nexibot_file_edit" }
    fn description(&self) -> &str {
        "Edit a file by replacing an exact string with a new string. The file MUST have been read with nexibot_file_read first. Returns a diff showing the change."
    }
    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": { "type": "string", "description": "Absolute path to the file" },
                "old_string": { "type": "string", "description": "The exact text to find and replace" },
                "new_string": { "type": "string", "description": "The replacement text" },
                "replace_all": { "type": "boolean", "description": "Replace all occurrences (default false)", "default": false }
            },
            "required": ["file_path", "old_string", "new_string"]
        })
    }

    async fn check_permissions(&self, input: &Value, _ctx: &ToolContext) -> PermissionDecision {
        match input["file_path"].as_str() {
            None => PermissionDecision::Deny("file_path is required".to_string()),
            Some(p) => {
                let path = Path::new(p);
                if is_dangerous_path(path) {
                    PermissionDecision::Deny(format!("Editing {} is not allowed", p))
                } else {
                    PermissionDecision::Allow
                }
            }
        }
    }

    async fn call(&self, input: Value, _ctx: ToolContext) -> ToolResult {
        let file_path = match input["file_path"].as_str() {
            Some(p) => PathBuf::from(p),
            None => return ToolResult::err("file_path is required"),
        };
        let old_string = match input["old_string"].as_str() {
            Some(s) => s.to_string(),
            None => return ToolResult::err("old_string is required"),
        };
        let new_string = match input["new_string"].as_str() {
            Some(s) => s.to_string(),
            None => return ToolResult::err("new_string is required"),
        };
        let replace_all = input["replace_all"].as_bool().unwrap_or(false);

        match apply_edit(&file_path, &old_string, &new_string, replace_all).await {
            Ok(diff) => ToolResult::ok(format!(
                "The file {} has been edited. Here's the diff:\n\n{}",
                file_path.display(), diff
            )),
            Err(e) => ToolResult::err(e),
        }
    }
}

const DANGEROUS_PATHS: &[&str] = &[
    ".git/config", ".bashrc", ".zshrc", ".bash_profile", ".gitconfig",
];

fn is_dangerous_path(path: &Path) -> bool {
    let s = path.to_string_lossy();
    DANGEROUS_PATHS.iter().any(|d| s.ends_with(d))
}

/// Normalize curly quotes to straight quotes for matching purposes.
pub fn normalize_quotes(s: &str) -> String {
    s.replace('\u{2018}', "'")
     .replace('\u{2019}', "'")
     .replace('\u{201C}', "\"")
     .replace('\u{201D}', "\"")
}

/// Find the actual string in file content, trying exact match then quote-normalized.
pub fn find_actual_string<'a>(content: &'a str, search: &str) -> Option<&'a str> {
    if content.contains(search) {
        return Some(search);
    }
    let norm_content = normalize_quotes(content);
    let norm_search = normalize_quotes(search);
    if let Some(pos) = norm_content.find(&norm_search) {
        Some(&content[pos..pos + search.len()])
    } else {
        None
    }
}

/// Generate a unified diff between old and new content.
pub fn generate_diff(path: &Path, old: &str, new: &str) -> String {
    let diff = TextDiff::from_lines(old, new);
    let filename = path.to_string_lossy();
    let mut output = String::new();

    for group in diff.grouped_ops(3) {
        for op in group {
            for change in diff.iter_inline_changes(&op) {
                let prefix = match change.tag() {
                    ChangeTag::Delete => "-",
                    ChangeTag::Insert => "+",
                    ChangeTag::Equal => " ",
                };
                for (_, value) in change.iter_strings_lossy() {
                    output.push_str(prefix);
                    output.push_str(&value);
                }
            }
        }
    }

    if output.is_empty() {
        "(no changes)".to_string()
    } else {
        format!("--- {}\n+++ {}\n{}", filename, filename, output)
    }
}

async fn apply_edit(
    path: &Path,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
) -> Result<String, String> {
    // Step 1: identical strings
    if old_string == new_string {
        return Err("old_string and new_string are identical — no change needed".to_string());
    }

    // Step 2: read file (or create new if old_string is empty)
    let original_content = if path.exists() {
        tokio::fs::read_to_string(path).await
            .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?
    } else if old_string.is_empty() {
        String::new()
    } else {
        return Err(format!("File not found: {}", path.display()));
    };

    // Step 3: creation on existing non-empty file
    if old_string.is_empty() && !original_content.trim().is_empty() {
        return Err("Cannot create file: it already exists and is non-empty. Use a specific old_string to edit it.".to_string());
    }

    // Step 4: notebook gate
    if path.extension().and_then(|e| e.to_str()) == Some("ipynb") {
        return Err("Use nexibot_notebook_edit to edit Jupyter notebooks.".to_string());
    }

    // Step 5: find the string (with quote normalization fallback)
    let actual = if old_string.is_empty() {
        ""
    } else {
        find_actual_string(&original_content, old_string)
            .ok_or_else(|| format!(
                "String not found in {}. The text may have changed since last read.",
                path.display()
            ))?
    };

    // Step 6: uniqueness check
    if !replace_all && !old_string.is_empty() {
        let count = original_content.matches(actual).count();
        if count > 1 {
            return Err(format!(
                "Found {} occurrences of the search string in {}. \
                 Provide more context to make it unique, or set replace_all=true.",
                count, path.display()
            ));
        }
    }

    // Step 7: apply replacement
    let new_content = if replace_all {
        original_content.replace(actual, new_string)
    } else {
        original_content.replacen(actual, new_string, 1)
    };

    // Step 8: write atomically (write to temp, rename)
    let tmp_path = path.with_extension("__nexibot_tmp__");
    tokio::fs::write(&tmp_path, &new_content).await
        .map_err(|e| format!("Failed to write {}: {}", path.display(), e))?;
    tokio::fs::rename(&tmp_path, path).await
        .map_err(|e| format!("Failed to rename tmp file: {}", e))?;

    // Step 9: generate diff
    Ok(generate_diff(path, &original_content, &new_content))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_quotes_curly_single() {
        let s = "\u{2018}hello\u{2019}";
        assert_eq!(normalize_quotes(s), "'hello'");
    }

    #[test]
    fn test_normalize_quotes_curly_double() {
        let s = "\u{201C}world\u{201D}";
        assert_eq!(normalize_quotes(s), "\"world\"");
    }

    #[test]
    fn test_find_actual_string_exact() {
        let content = "fn main() { println!(\"hello\"); }";
        assert!(find_actual_string(content, "println!").is_some());
    }

    #[test]
    fn test_find_actual_string_normalized() {
        let content = "let x = \u{201C}hello\u{201D};";
        let search = "let x = \"hello\";";
        assert!(find_actual_string(content, search).is_some());
    }

    #[test]
    fn test_find_actual_string_missing() {
        let content = "fn main() {}";
        assert!(find_actual_string(content, "fn other()").is_none());
    }

    #[test]
    fn test_generate_diff_shows_change() {
        let old = "fn hello() {\n    println!(\"old\");\n}\n";
        let new = "fn hello() {\n    println!(\"new\");\n}\n";
        let diff = generate_diff(Path::new("test.rs"), old, new);
        assert!(diff.contains("-") || diff.contains("+"));
        assert!(diff.contains("old") || diff.contains("new"));
    }

    #[tokio::test]
    async fn test_apply_edit_basic_replacement() {
        use std::io::Write;
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        write!(tmp, "fn hello() {{\n    let x = 1;\n}}\n").unwrap();
        let path = tmp.path().to_path_buf();

        let result = apply_edit(&path, "let x = 1;", "let x = 42;", false).await;
        assert!(result.is_ok());
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(content.contains("let x = 42;"));
        assert!(!content.contains("let x = 1;"));
    }

    #[tokio::test]
    async fn test_apply_edit_rejects_identical_strings() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let result = apply_edit(tmp.path(), "same", "same", false).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("identical"));
    }

    #[tokio::test]
    async fn test_apply_edit_rejects_multiple_matches() {
        use std::io::Write;
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        write!(tmp, "foo\nfoo\nfoo\n").unwrap();
        let result = apply_edit(tmp.path(), "foo", "bar", false).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("3 occurrences"));
    }

    #[tokio::test]
    async fn test_apply_edit_replace_all() {
        use std::io::Write;
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        write!(tmp, "foo\nfoo\nfoo\n").unwrap();
        let result = apply_edit(tmp.path(), "foo", "bar", true).await;
        assert!(result.is_ok());
        let content = tokio::fs::read_to_string(tmp.path()).await.unwrap();
        assert_eq!(content, "bar\nbar\nbar\n");
    }

    #[tokio::test]
    async fn test_apply_edit_rejects_notebook() {
        let tmp = tempfile::Builder::new().suffix(".ipynb").tempfile().unwrap();
        let result = apply_edit(tmp.path(), "old", "new", false).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("nexibot_notebook_edit"));
    }

    #[tokio::test]
    async fn test_apply_edit_creates_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new_file.rs");
        let result = apply_edit(&path, "", "fn main() {}\n", false).await;
        assert!(result.is_ok());
        assert!(path.exists());
    }
}
