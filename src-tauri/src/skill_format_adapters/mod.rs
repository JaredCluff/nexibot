//! External skill format adapters.
//!
//! Detects and converts skill definitions from OpenClaw, Codex, and Claude
//! formats into NexiBot's native SKILL.md format.

pub mod claude;
pub mod codex;
pub mod openclaw;

use anyhow::Result;
use std::path::Path;

/// Result of converting an external skill format to NexiBot format.
#[derive(Debug, Clone)]
pub struct ConvertedSkill {
    /// Skill name.
    pub name: String,
    /// Skill description.
    pub description: String,
    /// Converted SKILL.md content.
    pub skill_md: String,
    /// Source format name.
    pub source_format: String,
    /// Original file path.
    pub source_path: std::path::PathBuf,
}

/// Trait for adapters that detect and convert external skill formats.
pub trait SkillFormatAdapter: Send + Sync {
    /// Name of the skill format (e.g., "openclaw", "codex", "claude").
    fn format_name(&self) -> &str;
    /// Check if the given directory contains a skill in this format.
    fn detect(&self, dir: &Path) -> bool;
    /// Convert the skill definition to NexiBot format.
    fn convert(&self, dir: &Path) -> Result<ConvertedSkill>;
}

/// Get all available format adapters.
pub fn all_adapters() -> Vec<Box<dyn SkillFormatAdapter>> {
    vec![
        Box::new(openclaw::OpenClawAdapter),
        Box::new(codex::CodexAdapter),
        Box::new(claude::ClaudeAdapter),
    ]
}

/// Scan a directory for external skill formats and convert them.
pub fn discover_and_convert(dir: &Path) -> Vec<ConvertedSkill> {
    let adapters = all_adapters();
    let mut results = Vec::new();

    if !dir.exists() || !dir.is_dir() {
        return results;
    }

    // Check if this directory itself is a skill
    for adapter in &adapters {
        if adapter.detect(dir) {
            match adapter.convert(dir) {
                Ok(skill) => {
                    tracing::info!(
                        "[SKILL_DISCOVER] Converted {} skill: {} at {:?}",
                        skill.source_format,
                        skill.name,
                        dir
                    );
                    results.push(skill);
                    return results; // Directory is a single skill
                }
                Err(e) => {
                    tracing::warn!(
                        "[SKILL_DISCOVER] Failed to convert {} skill at {:?}: {}",
                        adapter.format_name(),
                        dir,
                        e
                    );
                }
            }
        }
    }

    // Otherwise, scan subdirectories
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return results,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        for adapter in &adapters {
            if adapter.detect(&path) {
                match adapter.convert(&path) {
                    Ok(skill) => {
                        tracing::info!(
                            "[SKILL_DISCOVER] Converted {} skill: {} at {:?}",
                            skill.source_format,
                            skill.name,
                            path
                        );
                        results.push(skill);
                        break; // Don't try other adapters for same dir
                    }
                    Err(e) => {
                        tracing::warn!(
                            "[SKILL_DISCOVER] Failed to convert {} skill at {:?}: {}",
                            adapter.format_name(),
                            path,
                            e
                        );
                    }
                }
            }
        }
    }

    results
}
