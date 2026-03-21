//! Claude Code skill format adapter.
//!
//! Detects skills by the presence of `.claude/CLAUDE.md` or a root `CLAUDE.md`.

use anyhow::{Context, Result};
use std::path::Path;

use super::{ConvertedSkill, SkillFormatAdapter};

pub struct ClaudeAdapter;

impl SkillFormatAdapter for ClaudeAdapter {
    fn format_name(&self) -> &str {
        "claude"
    }

    fn detect(&self, dir: &Path) -> bool {
        dir.join(".claude").join("CLAUDE.md").exists() || dir.join("CLAUDE.md").exists()
    }

    fn convert(&self, dir: &Path) -> Result<ConvertedSkill> {
        let claude_md_path = if dir.join(".claude").join("CLAUDE.md").exists() {
            dir.join(".claude").join("CLAUDE.md")
        } else {
            dir.join("CLAUDE.md")
        };

        let content =
            std::fs::read_to_string(&claude_md_path).context("Failed to read CLAUDE.md")?;

        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("claude-skill")
            .to_string();

        // Extract description from first heading or first paragraph
        let description = extract_first_paragraph(&content);

        // Convert CLAUDE.md to SKILL.md format
        let skill_md = format!(
            "---\nname: {}\ndescription: {}\nsource_format: claude\n---\n\n{}",
            name,
            description
                .replace('\n', " ")
                .chars()
                .take(200)
                .collect::<String>(),
            content
        );

        Ok(ConvertedSkill {
            name,
            description,
            skill_md,
            source_format: "claude".to_string(),
            source_path: dir.to_path_buf(),
        })
    }
}

fn extract_first_paragraph(content: &str) -> String {
    let mut lines = content.lines().peekable();
    let mut paragraph = String::new();

    // Skip YAML frontmatter if present
    if lines.peek().map(|l| l.starts_with("---")).unwrap_or(false) {
        lines.next();
        for line in lines.by_ref() {
            if line.starts_with("---") {
                break;
            }
        }
    }

    // Skip headings, find first non-empty paragraph
    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !paragraph.is_empty() {
                break; // End of first paragraph
            }
            continue;
        }
        if trimmed.starts_with('#') {
            if !paragraph.is_empty() {
                break;
            }
            continue;
        }
        if !paragraph.is_empty() {
            paragraph.push(' ');
        }
        paragraph.push_str(trimmed);
    }

    paragraph
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_detect_claude_md_root() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("CLAUDE.md"), "# Project\n").unwrap();

        let adapter = ClaudeAdapter;
        assert!(adapter.detect(dir.path()));
    }

    #[test]
    fn test_detect_dot_claude() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join(".claude")).unwrap();
        fs::write(dir.path().join(".claude/CLAUDE.md"), "# Project\n").unwrap();

        let adapter = ClaudeAdapter;
        assert!(adapter.detect(dir.path()));
    }

    #[test]
    fn test_no_detect_empty() {
        let dir = TempDir::new().unwrap();
        let adapter = ClaudeAdapter;
        assert!(!adapter.detect(dir.path()));
    }

    #[test]
    fn test_convert_claude_md() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("CLAUDE.md"),
            "# My Project\n\nThis is a test project for Claude.\n",
        )
        .unwrap();

        let adapter = ClaudeAdapter;
        let result = adapter.convert(dir.path()).unwrap();
        assert_eq!(result.source_format, "claude");
        assert!(result.skill_md.contains("source_format: claude"));
        assert!(result.description.contains("test project"));
    }
}
