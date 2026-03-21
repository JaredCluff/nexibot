//! OpenClaw skill format adapter.
//!
//! Detects skills by the presence of `plugin.json` with a skill type
//! or an existing `SKILL.md` file.

use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;

use super::{ConvertedSkill, SkillFormatAdapter};

pub struct OpenClawAdapter;

impl SkillFormatAdapter for OpenClawAdapter {
    fn format_name(&self) -> &str {
        "openclaw"
    }

    fn detect(&self, dir: &Path) -> bool {
        let plugin_json = dir.join("plugin.json");
        if plugin_json.exists() {
            if let Ok(content) = std::fs::read_to_string(&plugin_json) {
                if let Ok(manifest) = serde_json::from_str::<Value>(&content) {
                    // Check if it's a skill-type plugin
                    if let Some(plugin_type) = manifest.get("type").and_then(|t| t.as_str()) {
                        return plugin_type == "skill" || plugin_type == "command";
                    }
                    // Any plugin.json with a "name" field qualifies
                    return manifest.get("name").is_some();
                }
            }
        }

        // Also detect existing SKILL.md (OpenClaw-compatible)
        dir.join("SKILL.md").exists()
    }

    fn convert(&self, dir: &Path) -> Result<ConvertedSkill> {
        // If SKILL.md already exists, just read it
        let skill_md_path = dir.join("SKILL.md");
        if skill_md_path.exists() {
            let content =
                std::fs::read_to_string(&skill_md_path).context("Failed to read SKILL.md")?;
            let name = dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
            return Ok(ConvertedSkill {
                name,
                description: extract_description(&content),
                skill_md: content,
                source_format: "openclaw".to_string(),
                source_path: dir.to_path_buf(),
            });
        }

        // Convert from plugin.json
        let plugin_json = dir.join("plugin.json");
        let content =
            std::fs::read_to_string(&plugin_json).context("Failed to read plugin.json")?;
        let manifest: Value =
            serde_json::from_str(&content).context("Failed to parse plugin.json")?;

        let name = manifest
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("unknown")
            .to_string();

        let description = manifest
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or("")
            .to_string();

        let version = manifest
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or("0.1.0");

        // Build SKILL.md from plugin.json
        let mut skill_md = format!(
            "---\nname: {}\ndescription: {}\nversion: {}\nsource_format: openclaw\n",
            name, description, version
        );

        // Add commands if present
        if let Some(commands) = manifest.get("commands").and_then(|c| c.as_array()) {
            skill_md.push_str("commands:\n");
            for cmd in commands {
                if let Some(cmd_name) = cmd.get("name").and_then(|n| n.as_str()) {
                    skill_md.push_str(&format!("  - {}\n", cmd_name));
                }
            }
        }

        skill_md.push_str("---\n\n");
        skill_md.push_str(&format!("# {}\n\n", name));
        skill_md.push_str(&format!("{}\n", description));

        // Include README.md content if present
        let readme_path = dir.join("README.md");
        if readme_path.exists() {
            if let Ok(readme) = std::fs::read_to_string(&readme_path) {
                skill_md.push_str("\n\n## Documentation\n\n");
                skill_md.push_str(&readme);
            }
        }

        Ok(ConvertedSkill {
            name,
            description,
            skill_md,
            source_format: "openclaw".to_string(),
            source_path: dir.to_path_buf(),
        })
    }
}

fn extract_description(skill_md: &str) -> String {
    // Try to extract description from YAML frontmatter
    if let Some(content) = skill_md.strip_prefix("---") {
        if let Some(end) = content.find("---") {
            let frontmatter = &content[..end];
            for line in frontmatter.lines() {
                if let Some(desc) = line.strip_prefix("description:") {
                    return desc.trim().to_string();
                }
            }
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_detect_plugin_json() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("plugin.json"),
            r#"{"name": "test-skill", "type": "skill"}"#,
        )
        .unwrap();

        let adapter = OpenClawAdapter;
        assert!(adapter.detect(dir.path()));
    }

    #[test]
    fn test_detect_skill_md() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("SKILL.md"), "# Test Skill\n").unwrap();

        let adapter = OpenClawAdapter;
        assert!(adapter.detect(dir.path()));
    }

    #[test]
    fn test_no_detect_empty() {
        let dir = TempDir::new().unwrap();
        let adapter = OpenClawAdapter;
        assert!(!adapter.detect(dir.path()));
    }

    #[test]
    fn test_convert_plugin_json() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("plugin.json"),
            r#"{"name": "web-search", "description": "Search the web", "version": "1.0.0"}"#,
        )
        .unwrap();

        let adapter = OpenClawAdapter;
        let result = adapter.convert(dir.path()).unwrap();
        assert_eq!(result.name, "web-search");
        assert_eq!(result.source_format, "openclaw");
        assert!(result.skill_md.contains("web-search"));
    }
}
