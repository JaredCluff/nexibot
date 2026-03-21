//! Codex skill format adapter.
//!
//! Detects skills by the presence of `codex.json` or `.codex/config.json`.

use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;

use super::{ConvertedSkill, SkillFormatAdapter};

pub struct CodexAdapter;

impl SkillFormatAdapter for CodexAdapter {
    fn format_name(&self) -> &str {
        "codex"
    }

    fn detect(&self, dir: &Path) -> bool {
        dir.join("codex.json").exists() || dir.join(".codex").join("config.json").exists()
    }

    fn convert(&self, dir: &Path) -> Result<ConvertedSkill> {
        let config_path = if dir.join("codex.json").exists() {
            dir.join("codex.json")
        } else {
            dir.join(".codex").join("config.json")
        };

        let content =
            std::fs::read_to_string(&config_path).context("Failed to read Codex config")?;
        let config: Value =
            serde_json::from_str(&content).context("Failed to parse Codex config")?;

        let name = config
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or_else(|| {
                dir.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("codex-skill")
            })
            .to_string();

        let description = config
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or("")
            .to_string();

        let mut skill_md = format!(
            "---\nname: {}\ndescription: {}\nsource_format: codex\n---\n\n# {}\n\n{}\n",
            name, description, name, description
        );

        // Include instructions if present
        if let Some(instructions) = config.get("instructions").and_then(|i| i.as_str()) {
            skill_md.push_str("\n## Instructions\n\n");
            skill_md.push_str(instructions);
            skill_md.push('\n');
        }

        // Include tools if listed
        if let Some(tools) = config.get("tools").and_then(|t| t.as_array()) {
            skill_md.push_str("\n## Tools\n\n");
            for tool in tools {
                if let Some(tool_name) = tool.as_str() {
                    skill_md.push_str(&format!("- {}\n", tool_name));
                }
            }
        }

        Ok(ConvertedSkill {
            name,
            description,
            skill_md,
            source_format: "codex".to_string(),
            source_path: dir.to_path_buf(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_detect_codex_json() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("codex.json"),
            r#"{"name": "codex-skill"}"#,
        )
        .unwrap();

        let adapter = CodexAdapter;
        assert!(adapter.detect(dir.path()));
    }

    #[test]
    fn test_detect_dot_codex() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join(".codex")).unwrap();
        fs::write(
            dir.path().join(".codex/config.json"),
            r#"{"name": "codex-skill"}"#,
        )
        .unwrap();

        let adapter = CodexAdapter;
        assert!(adapter.detect(dir.path()));
    }

    #[test]
    fn test_no_detect_empty() {
        let dir = TempDir::new().unwrap();
        let adapter = CodexAdapter;
        assert!(!adapter.detect(dir.path()));
    }
}
