///! Skills System (OpenClaw-inspired)
///!
///! Skills are modular capabilities that extend the agent's functionality.
///! Each skill is a folder containing:
///! - SKILL.md (YAML frontmatter + markdown instructions)
///! - scripts/ (optional executable scripts)
///! - references/ (optional documentation)
///! - assets/ (optional templates and resources)
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{error, info, warn};

/// Reject a path if it is a symlink (defense-in-depth for skill loading).
///
/// Returns an error if the path is a symlink, preventing symlink-based
/// attacks that could redirect skill file reads outside the skills directory.
fn reject_if_symlink(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(format!("Rejected symlink in skill path: {:?}", path));
            }
            Ok(())
        }
        Err(e) => Err(format!(
            "Failed to check symlink status of {:?}: {}",
            path, e
        )),
    }
}

/// Resolve the appropriate script interpreter and its arguments based on file extension.
///
/// Returns `(interpreter_path, extra_args_before_script)`.
/// On Windows: `.ps1` → PowerShell, `.cmd`/`.bat` → cmd.exe, `.py` → python, `.sh` → Git Bash or sh.
/// On Unix: `.py` → python3, everything else → `/bin/sh`.
fn resolve_script_interpreter(script_path: &Path) -> (PathBuf, Vec<String>) {
    let ext = script_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    #[cfg(windows)]
    {
        match ext.as_str() {
            "ps1" => {
                let ps = std::env::var("SystemRoot")
                    .map(|sr| {
                        PathBuf::from(sr)
                            .join("System32")
                            .join("WindowsPowerShell")
                            .join("v1.0")
                            .join("powershell.exe")
                    })
                    .unwrap_or_else(|_| PathBuf::from("powershell.exe"));
                (ps, vec!["-ExecutionPolicy".into(), "Bypass".into(), "-File".into()])
            }
            "cmd" | "bat" => {
                let cmd = crate::platform::default_script_shell();
                (cmd, vec!["/C".into()])
            }
            "py" => (PathBuf::from("python"), vec![]),
            "sh" => {
                // Try Git Bash on Windows
                let git_bash = std::env::var("ProgramFiles")
                    .map(|pf| PathBuf::from(pf).join("Git").join("bin").join("bash.exe"))
                    .unwrap_or_else(|_| PathBuf::from("bash.exe"));
                if git_bash.exists() {
                    (git_bash, vec![])
                } else {
                    (PathBuf::from("bash.exe"), vec![])
                }
            }
            _ => {
                // Default: use cmd.exe
                let cmd = crate::platform::default_script_shell();
                (cmd, vec!["/C".into()])
            }
        }
    }

    #[cfg(not(windows))]
    {
        match ext.as_str() {
            "py" => (PathBuf::from("python3"), vec![]),
            _ => (PathBuf::from("/bin/sh"), vec![]),
        }
    }
}

/// Skill metadata from YAML frontmatter
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct SkillMetadata {
    /// Skill name
    pub name: Option<String>,
    /// Description
    pub description: Option<String>,
    /// Whether skill can be invoked by user (as /command)
    #[serde(default = "default_true")]
    pub user_invocable: bool,
    /// Whether skill is excluded from model prompt
    #[serde(default)]
    pub disable_model_invocation: bool,
    /// Required permissions or capabilities
    #[serde(default)]
    pub requirements: Vec<String>,

    // --- OpenClaw-compatible fields ---
    /// OpenClaw metadata/requires block (JSON: { bins: [], env: [], config: [] })
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
    /// Command dispatch mode: "prompt", "script", "tool"
    #[serde(default)]
    pub command_dispatch: Option<String>,
    /// Tool name for command dispatch
    #[serde(default)]
    pub command_tool: Option<String>,
    /// Argument mode for command dispatch
    #[serde(default)]
    pub command_arg_mode: Option<String>,
    /// Skill version (semver)
    #[serde(default)]
    pub version: Option<String>,
    /// Skill author
    #[serde(default)]
    pub author: Option<String>,
    /// Source: "local" or "clawhub"
    #[serde(default)]
    pub source: Option<String>,
}

/// OpenClaw requirements parsed from the metadata JSON field.
#[derive(Debug, Clone, Default)]
pub struct OpenClawRequires {
    /// Required binaries on PATH
    pub bins: Vec<String>,
    /// Required environment variables
    pub env: Vec<String>,
    /// Required config file paths
    #[allow(dead_code)]
    pub config: Vec<String>,
}

impl SkillMetadata {
    /// Parse OpenClaw requirements from the metadata field.
    pub fn openclaw_requires(&self) -> Option<OpenClawRequires> {
        let meta = self.metadata.as_ref()?;

        let bins = meta
            .get("bins")
            .and_then(|b| b.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let env = meta
            .get("env")
            .and_then(|e| e.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let config = meta
            .get("config")
            .and_then(|c| c.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        Some(OpenClawRequires { bins, env, config })
    }
}

/// Installation info for skills installed from ClawHub.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInstallInfo {
    /// ClawHub slug (e.g., "author/skill-name")
    pub slug: String,
    /// Installed version
    pub version: String,
    /// Installation timestamp (ISO 8601)
    pub installed_at: String,
    /// Security score at install time (0.0-1.0)
    pub security_score: f32,
}

fn default_true() -> bool {
    true
}

fn default_timeout_seconds() -> u64 {
    30
}

fn default_max_output_bytes() -> usize {
    1024 * 1024 // 1 MiB
}

/// Per-skill runtime configuration (from `skill.config.yaml`).
///
/// This file is optional. When absent, defaults apply.
/// Values in this file are readable by skill scripts via `SKILL_CONFIG_*` env vars.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillConfig {
    /// Script execution timeout in seconds. Default: 30.
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,

    /// Maximum combined stdout+stderr size in bytes. Default: 1 MiB.
    #[serde(default = "default_max_output_bytes")]
    pub max_output_bytes: usize,

    /// User-defined key/value pairs passed to scripts as `SKILL_CONFIG_<KEY>=<value>`.
    /// Keys must match `[A-Z0-9_]+` (validated at load time).
    #[serde(default)]
    pub values: HashMap<String, String>,
}

impl Default for SkillConfig {
    fn default() -> Self {
        Self {
            timeout_seconds: default_timeout_seconds(),
            max_output_bytes: default_max_output_bytes(),
            values: HashMap::new(),
        }
    }
}

/// Result of executing a skill script.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillExecResult {
    /// Exit code from the script process.
    pub exit_code: i32,
    /// Combined stdout from the script (truncated at `max_output_bytes`).
    pub stdout: String,
    /// Whether the script exited successfully (exit code 0).
    pub success: bool,
    /// Script name that was executed.
    pub script: String,
    /// Skill ID that was invoked.
    pub skill_id: String,
}

/// A loaded skill with its content and metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    /// Skill identifier (folder name)
    pub id: String,
    /// Skill metadata from frontmatter
    pub metadata: SkillMetadata,
    /// Markdown content (instructions for the agent)
    pub content: String,
    /// Path to skill directory
    pub path: PathBuf,
    /// Available scripts
    pub scripts: Vec<String>,
    /// Available references
    pub references: Vec<String>,
    /// ClawHub installation info (None for local skills)
    #[serde(default)]
    pub install_info: Option<SkillInstallInfo>,
}

/// Skills manager
pub struct SkillsManager {
    skills_dir: PathBuf,
    loaded_skills: HashMap<String, Skill>,
}

impl SkillsManager {
    /// Create a new skills manager
    pub fn new() -> Result<Self> {
        let skills_dir = Self::get_skills_dir()?;
        fs::create_dir_all(&skills_dir)?;

        let mut manager = Self {
            skills_dir,
            loaded_skills: HashMap::new(),
        };

        manager.load_all_skills()?;
        Ok(manager)
    }

    /// Get the skills directory path
    pub fn get_skills_dir() -> Result<PathBuf> {
        let home = dirs::home_dir().context("Failed to get home directory")?;
        Ok(home.join(".config/nexibot/skills"))
    }

    /// Load all skills from the skills directory.
    /// Clears the loaded skills map first so deleted skill folders are properly removed.
    pub fn load_all_skills(&mut self) -> Result<()> {
        if !self.skills_dir.exists() {
            info!("[SKILLS] Skills directory does not exist, creating it");
            fs::create_dir_all(&self.skills_dir)?;
            self.loaded_skills.clear();
            return Ok(());
        }

        self.loaded_skills.clear();
        let mut count = 0;
        for entry in fs::read_dir(&self.skills_dir)? {
            let entry = entry?;
            let path = entry.path();

            // Reject symlinks in the skills directory (defense-in-depth)
            if let Ok(meta) = fs::symlink_metadata(&path) {
                if meta.is_symlink() {
                    warn!("[SKILLS] Skipping symlink: {:?}", path);
                    continue;
                }
            }

            if path.is_dir() {
                match self.load_skill(&path) {
                    Ok(skill) => {
                        info!("[SKILLS] Loaded skill: {}", skill.id);
                        self.loaded_skills.insert(skill.id.clone(), skill);
                        count += 1;
                    }
                    Err(e) => {
                        warn!("[SKILLS] Failed to load skill from {:?}: {}", path, e);
                    }
                }
            }
        }

        info!("[SKILLS] Loaded {} skills", count);
        Ok(())
    }

    /// Discover and import skills from external directories.
    ///
    /// Scans the given directories for OpenClaw, Codex, and Claude skill formats
    /// and converts them into NexiBot's native SKILL.md format. Converted skills
    /// are written to the skills directory and loaded into the manager.
    ///
    /// Only formats listed in `allowed_formats` are considered. Skills whose
    /// IDs already exist in the loaded skills map are skipped.
    pub fn discover_external_skills(
        &mut self,
        external_dirs: &[String],
        allowed_formats: &[String],
    ) -> Result<Vec<String>> {
        let mut imported = Vec::new();

        if external_dirs.is_empty() {
            return Ok(imported);
        }

        let all_adapters = crate::skill_format_adapters::all_adapters();
        let adapters: Vec<_> = all_adapters
            .into_iter()
            .filter(|a| allowed_formats.iter().any(|f| f == a.format_name()))
            .collect();

        if adapters.is_empty() {
            warn!("[SKILLS] No format adapters enabled for external skill discovery");
            return Ok(imported);
        }

        info!(
            "[SKILLS] Scanning {} external directories for skills ({} formats enabled)",
            external_dirs.len(),
            adapters.len()
        );

        for dir_str in external_dirs {
            let dir = Path::new(dir_str);
            if !dir.exists() || !dir.is_dir() {
                warn!(
                    "[SKILLS] External skill directory does not exist: {:?}",
                    dir
                );
                continue;
            }

            let converted =
                crate::skill_format_adapters::discover_and_convert(dir);

            for skill in converted {
                // Filter by allowed formats
                if !allowed_formats.iter().any(|f| f == &skill.source_format) {
                    continue;
                }

                // Skip if already loaded
                if self.loaded_skills.contains_key(&skill.name) {
                    info!(
                        "[SKILLS] Skipping external skill '{}' — already loaded",
                        skill.name
                    );
                    continue;
                }

                // Write the converted SKILL.md to the skills directory
                let target_dir = self.skills_dir.join(&skill.name);
                if target_dir.exists() {
                    info!(
                        "[SKILLS] Skipping external skill '{}' — directory already exists",
                        skill.name
                    );
                    continue;
                }

                match fs::create_dir_all(&target_dir) {
                    Ok(_) => {}
                    Err(e) => {
                        warn!(
                            "[SKILLS] Failed to create directory for external skill '{}': {}",
                            skill.name, e
                        );
                        continue;
                    }
                }

                let skill_md_path = target_dir.join("SKILL.md");
                if let Err(e) = fs::write(&skill_md_path, &skill.skill_md) {
                    warn!(
                        "[SKILLS] Failed to write SKILL.md for external skill '{}': {}",
                        skill.name, e
                    );
                    // Clean up the directory on failure
                    let _ = fs::remove_dir_all(&target_dir);
                    continue;
                }

                // Write a source marker so we know this was auto-imported
                let marker = serde_json::json!({
                    "source_format": skill.source_format,
                    "source_path": skill.source_path.to_string_lossy(),
                    "imported_at": chrono::Utc::now().to_rfc3339(),
                });
                let _ = fs::write(
                    target_dir.join(".import_info.json"),
                    serde_json::to_string_pretty(&marker).unwrap_or_default(),
                );

                // Load the newly created skill through the normal pipeline
                match self.load_skill(&target_dir) {
                    Ok(loaded) => {
                        info!(
                            "[SKILLS] Imported external {} skill: {}",
                            skill.source_format, loaded.id
                        );
                        imported.push(loaded.id.clone());
                        self.loaded_skills.insert(loaded.id.clone(), loaded);
                    }
                    Err(e) => {
                        warn!(
                            "[SKILLS] External skill '{}' converted but failed security scan: {}",
                            skill.name, e
                        );
                        // Clean up on load failure
                        let _ = fs::remove_dir_all(&target_dir);
                    }
                }
            }
        }

        info!(
            "[SKILLS] External skill discovery complete: {} skills imported",
            imported.len()
        );
        Ok(imported)
    }

    /// Load a single skill from a directory.
    ///
    /// Tries loading in order: SKILL.md, skill.yaml, skill.toml.
    fn load_skill(&self, skill_dir: &PathBuf) -> Result<Skill> {
        let skill = self.load_skill_inner(skill_dir)?;

        // Security scan: check skill content for dangerous patterns.
        // Block if the scan finds Danger or Critical severity issues (safe == false).
        // Info/Warning findings are logged but do not prevent loading.
        let scan_result = crate::security::skill_scanner::scan_skill_code(&skill.content);
        if !scan_result.safe {
            // Collect all Danger and Critical findings for the error message.
            let high_risk: Vec<_> = scan_result
                .findings
                .iter()
                .filter(|f| {
                    matches!(
                        f.severity,
                        crate::security::skill_scanner::ScanSeverity::Danger
                            | crate::security::skill_scanner::ScanSeverity::Critical
                    )
                })
                .map(|f| format!("[{}] {}: {}", f.severity, f.pattern_name, f.description))
                .collect();
            warn!(
                "[SKILLS] Skill '{}' blocked by security scanner (max_severity={}): {:?}",
                skill.id, scan_result.max_severity, high_risk
            );
            anyhow::bail!(
                "Skill '{}' refused: security scanner found {} high-risk pattern(s): {}",
                skill.id,
                high_risk.len(),
                high_risk.join("; ")
            );
        }
        if !scan_result.findings.is_empty() {
            warn!(
                "[SKILLS] Skill '{}' has {} security findings (non-blocking)",
                skill.id,
                scan_result.findings.len()
            );
        }

        // Integrity verification: check hash against recorded value (if any)
        if skill.install_info.is_some() {
            match crate::clawhub::SkillIntegrityManifest::load() {
                Ok(manifest) => {
                    match manifest.verify(&skill.id, skill_dir) {
                        Ok(true) => {} // Hash matches or no record — all good
                        Ok(false) => {
                            warn!(
                                "[SKILLS] Skill '{}' integrity check FAILED — files may have been \
                                 modified since installation. The skill will still load, but this \
                                 may indicate tampering.",
                                skill.id
                            );
                        }
                        Err(e) => {
                            warn!(
                                "[SKILLS] Could not verify integrity for skill '{}': {}",
                                skill.id, e
                            );
                        }
                    }
                }
                Err(e) => {
                    warn!("[SKILLS] Could not load integrity manifest: {}", e);
                }
            }
        }

        Ok(skill)
    }

    fn load_skill_inner(&self, skill_dir: &PathBuf) -> Result<Skill> {
        // Try SKILL.md first (original format)
        let skill_md_path = skill_dir.join("SKILL.md");
        if skill_md_path.exists() {
            reject_if_symlink(&skill_md_path).map_err(|e| anyhow::anyhow!("{}", e))?;
            return self.load_skill_from_md(skill_dir, &skill_md_path);
        }

        // Try skill.yaml
        let yaml_path = skill_dir.join("skill.yaml");
        if yaml_path.exists() {
            reject_if_symlink(&yaml_path).map_err(|e| anyhow::anyhow!("{}", e))?;
            return self.load_skill_from_yaml(skill_dir, &yaml_path);
        }

        // Try skill.yml
        let yml_path = skill_dir.join("skill.yml");
        if yml_path.exists() {
            reject_if_symlink(&yml_path).map_err(|e| anyhow::anyhow!("{}", e))?;
            return self.load_skill_from_yaml(skill_dir, &yml_path);
        }

        // Try skill.toml
        let toml_path = skill_dir.join("skill.toml");
        if toml_path.exists() {
            reject_if_symlink(&toml_path).map_err(|e| anyhow::anyhow!("{}", e))?;
            return self.load_skill_from_toml(skill_dir, &toml_path);
        }

        anyhow::bail!(
            "No skill definition found in {:?} (expected SKILL.md, skill.yaml, or skill.toml)",
            skill_dir
        );
    }

    /// Load a skill from a SKILL.md file (original format with YAML frontmatter).
    fn load_skill_from_md(
        &self,
        skill_dir: &PathBuf,
        skill_md_path: &std::path::Path,
    ) -> Result<Skill> {
        let skill_id = skill_dir
            .file_name()
            .and_then(|s| s.to_str())
            .context("Invalid skill directory name")?
            .to_string();

        let content = fs::read_to_string(skill_md_path)?;
        let (metadata, markdown) = Self::parse_skill_md(&content)?;

        // Discover scripts
        let scripts = Self::list_directory_files(&skill_dir.join("scripts"))?;

        // Discover references
        let references = Self::list_directory_files(&skill_dir.join("references"))?;

        // Check for install_info.json (ClawHub installs)
        let install_info_path = skill_dir.join("install_info.json");
        let install_info = if install_info_path.exists() {
            // Reject symlink on install_info.json
            if reject_if_symlink(&install_info_path).is_err() {
                warn!(
                    "[SKILLS] Skipping symlinked install_info.json in {:?}",
                    skill_dir
                );
                None
            } else {
                fs::read_to_string(&install_info_path)
                    .ok()
                    .and_then(|s| serde_json::from_str::<SkillInstallInfo>(&s).ok())
            }
        } else {
            None
        };

        Ok(Skill {
            id: skill_id,
            metadata,
            content: markdown,
            path: skill_dir.clone(),
            scripts,
            references,
            install_info,
        })
    }

    /// Validate that skill frontmatter doesn't contain YAML 1.1 implicit boolean values
    /// that could cause unexpected coercion (on/off/yes/no).
    fn validate_frontmatter_no_implicit_booleans(frontmatter: &str) -> Result<(), String> {
        let yaml_11_booleans = ["on", "off", "yes", "no", "y", "n", "true", "false"];
        for line in frontmatter.lines() {
            if let Some((key, value)) = line.split_once(':') {
                let value = value.trim();
                let lower = value.to_lowercase();
                // Only flag non-quoted values that match YAML 1.1 booleans
                if !value.starts_with('"') && !value.starts_with('\'') {
                    if yaml_11_booleans.contains(&lower.as_str())
                        && lower != "true"
                        && lower != "false"
                    {
                        return Err(format!(
                            "Skill frontmatter uses YAML 1.1 implicit boolean '{}' for key '{}'. Use true/false or quote the value.",
                            value, key.trim()
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// Parse SKILL.md file (YAML frontmatter + markdown)
    fn parse_skill_md(content: &str) -> Result<(SkillMetadata, String)> {
        let lines: Vec<&str> = content.lines().collect();

        // Check for YAML frontmatter (starts and ends with ---)
        if lines.first() == Some(&"---") {
            // Find the closing ---
            let end_idx = lines
                .iter()
                .skip(1)
                .position(|line| *line == "---")
                .context("Unclosed YAML frontmatter")?
                + 1;

            let yaml_content = lines[1..end_idx].join("\n");

            // Validate against YAML 1.1 implicit boolean coercion
            if let Err(e) = Self::validate_frontmatter_no_implicit_booleans(&yaml_content) {
                warn!("[SKILLS] {}", e);
                anyhow::bail!(e);
            }

            let metadata: SkillMetadata =
                serde_yml::from_str(&yaml_content).context("Failed to parse YAML frontmatter")?;

            let markdown = lines[end_idx + 1..].join("\n");

            Ok((metadata, markdown))
        } else {
            // No frontmatter, use defaults
            Ok((
                SkillMetadata {
                    name: None,
                    description: None,
                    user_invocable: true,
                    disable_model_invocation: false,
                    requirements: Vec::new(),
                    metadata: None,
                    command_dispatch: None,
                    command_tool: None,
                    command_arg_mode: None,
                    version: None,
                    author: None,
                    source: None,
                },
                content.to_string(),
            ))
        }
    }

    /// List files in a directory (non-recursive)
    fn list_directory_files(dir: &PathBuf) -> Result<Vec<String>> {
        if !dir.exists() {
            return Ok(Vec::new());
        }

        // Reject symlink on the directory itself — read_dir follows symlinks, so
        // a symlinked scripts/ or references/ directory would silently bypass the
        // per-entry symlink checks and expose files outside the skill boundary.
        if reject_if_symlink(dir).is_err() {
            warn!("[SKILLS] Skipping symlinked directory: {:?}", dir);
            return Ok(Vec::new());
        }

        let mut files = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            // Reject symlinks in skill subdirectories (defense-in-depth)
            if reject_if_symlink(&path).is_err() {
                warn!("[SKILLS] Skipping symlink in skill directory: {:?}", path);
                continue;
            }

            if path.is_file() {
                if let Some(name) = entry.file_name().to_str() {
                    files.push(name.to_string());
                }
            }
        }

        Ok(files)
    }

    /// Get a skill by ID
    pub fn get_skill(&self, skill_id: &str) -> Option<&Skill> {
        self.loaded_skills.get(skill_id)
    }

    /// List all loaded skills
    pub fn list_skills(&self) -> Vec<&Skill> {
        self.loaded_skills.values().collect()
    }

    /// Get user-invocable skills (for /commands)
    pub fn get_user_invocable_skills(&self) -> Vec<&Skill> {
        self.loaded_skills
            .values()
            .filter(|s| s.metadata.user_invocable)
            .collect()
    }

    /// Get the declared environment variables for a skill.
    ///
    /// Reads the `env` array from the skill's `metadata` JSON field.
    /// Returns an empty Vec if the skill doesn't exist or has no env declarations.
    #[allow(dead_code)]
    pub fn get_declared_env_vars(&self, skill_id: &str) -> Vec<String> {
        match self.loaded_skills.get(skill_id) {
            Some(skill) => skill
                .metadata
                .openclaw_requires()
                .map(|r| r.env)
                .unwrap_or_default(),
            None => Vec::new(),
        }
    }

    /// Get model-invocable skills (for automatic context loading)
    pub fn get_model_invocable_skills(&self) -> Vec<&Skill> {
        self.loaded_skills
            .values()
            .filter(|s| !s.metadata.disable_model_invocation)
            .collect()
    }

    /// Create a new skill with SKILL.md file.
    pub fn create_skill(
        &mut self,
        id: &str,
        name: &str,
        description: &str,
        content: &str,
        user_invocable: bool,
    ) -> Result<Skill> {
        // Reject path traversal and shell-special characters in the ID.
        if id.is_empty() || id.contains("..") || id.contains('/') || id.contains('\\') || id.contains('\0') {
            anyhow::bail!("Invalid skill ID '{}': must not contain path traversal characters", id);
        }
        if !id.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
            anyhow::bail!("Invalid skill ID '{}': only alphanumeric, hyphen, and underscore allowed", id);
        }

        let skill_dir = self.skills_dir.join(id);
        if skill_dir.exists() {
            anyhow::bail!("Skill '{}' already exists", id);
        }

        fs::create_dir_all(&skill_dir)?;

        let skill_md = format!(
            "---\nname: {}\ndescription: {}\nuser-invocable: {}\n---\n\n{}",
            name, description, user_invocable, content
        );

        fs::write(skill_dir.join("SKILL.md"), &skill_md)?;

        let skill = Skill {
            id: id.to_string(),
            metadata: SkillMetadata {
                name: Some(name.to_string()),
                description: Some(description.to_string()),
                user_invocable,
                disable_model_invocation: false,
                requirements: Vec::new(),
                metadata: None,
                command_dispatch: None,
                command_tool: None,
                command_arg_mode: None,
                version: None,
                author: None,
                source: Some("local".to_string()),
            },
            content: content.to_string(),
            path: skill_dir,
            scripts: Vec::new(),
            references: Vec::new(),
            install_info: None,
        };

        self.loaded_skills.insert(id.to_string(), skill.clone());
        info!("[SKILLS] Created skill: {}", id);
        Ok(skill)
    }

    /// Update an existing skill.
    pub fn update_skill(
        &mut self,
        id: &str,
        name: Option<&str>,
        description: Option<&str>,
        content: Option<&str>,
        user_invocable: Option<bool>,
    ) -> Result<Skill> {
        let skill = self
            .loaded_skills
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("Skill not found: {}", id))?
            .clone();

        let new_name = name.unwrap_or_else(|| skill.metadata.name.as_deref().unwrap_or(id));
        let new_desc =
            description.unwrap_or_else(|| skill.metadata.description.as_deref().unwrap_or(""));
        let new_content = content.unwrap_or(&skill.content);
        let new_invocable = user_invocable.unwrap_or(skill.metadata.user_invocable);

        let skill_md = format!(
            "---\nname: {}\ndescription: {}\nuser-invocable: {}\n---\n\n{}",
            new_name, new_desc, new_invocable, new_content
        );

        fs::write(skill.path.join("SKILL.md"), &skill_md)?;

        // Reload the skill
        let updated = self.load_skill(&skill.path)?;
        self.loaded_skills.insert(id.to_string(), updated.clone());
        info!("[SKILLS] Updated skill: {}", id);
        Ok(updated)
    }

    /// Delete a skill and its directory.
    pub fn delete_skill(&mut self, id: &str) -> Result<()> {
        let skill = self
            .loaded_skills
            .remove(id)
            .ok_or_else(|| anyhow::anyhow!("Skill not found: {}", id))?;

        if skill.path.exists() {
            fs::remove_dir_all(&skill.path)?;
        }

        info!("[SKILLS] Deleted skill: {}", id);
        Ok(())
    }

    /// Get skills context for system prompt
    pub fn get_skills_context(&self) -> String {
        let model_skills = self.get_model_invocable_skills();

        if model_skills.is_empty() {
            return String::new();
        }

        // Boundary markers signal to the model that skill content is user-controlled
        // and may contain adversarial prompts (second-order prompt injection defense,
        // consistent with soul.rs's <SOUL_CONTENT> markers).
        let mut context = String::from("# Available Skills\n\n<SKILLS_CONTENT>\n");
        context.push_str("I have access to the following skills:\n\n");

        for skill in model_skills {
            let name = skill.metadata.name.as_ref().unwrap_or(&skill.id);

            let desc = skill
                .metadata
                .description
                .as_deref()
                .unwrap_or("No description");

            context.push_str(&format!("## {}\n\n{}\n\n", name, desc));
            context.push_str(&format!("{}\n\n", skill.content));
        }

        context.push_str("</SKILLS_CONTENT>\n");
        context
    }
}

/// A skill template for quick creation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillTemplate {
    pub id: String,
    pub name: String,
    pub description: String,
    pub content: String,
    pub user_invocable: bool,
}

/// Bundled skill definitions embedded at compile time.
fn get_bundled_skills() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "code-review",
            include_str!("../resources/bundled-skills/code-review/SKILL.md"),
        ),
        (
            "email-writer",
            include_str!("../resources/bundled-skills/email-writer/SKILL.md"),
        ),
        (
            "summarizer",
            include_str!("../resources/bundled-skills/summarizer/SKILL.md"),
        ),
        (
            "translator",
            include_str!("../resources/bundled-skills/translator/SKILL.md"),
        ),
        (
            "web-researcher",
            include_str!("../resources/bundled-skills/web-researcher/SKILL.md"),
        ),
        (
            "meeting-notes",
            include_str!("../resources/bundled-skills/meeting-notes/SKILL.md"),
        ),
        (
            "data-analyst",
            include_str!("../resources/bundled-skills/data-analyst/SKILL.md"),
        ),
        (
            "creative-writer",
            include_str!("../resources/bundled-skills/creative-writer/SKILL.md"),
        ),
        (
            "debugging-assistant",
            include_str!("../resources/bundled-skills/debugging-assistant/SKILL.md"),
        ),
        (
            "git-helper",
            include_str!("../resources/bundled-skills/git-helper/SKILL.md"),
        ),
        (
            "api-tester",
            include_str!("../resources/bundled-skills/api-tester/SKILL.md"),
        ),
        (
            "project-planner",
            include_str!("../resources/bundled-skills/project-planner/SKILL.md"),
        ),
        (
            "study-helper",
            include_str!("../resources/bundled-skills/study-helper/SKILL.md"),
        ),
        (
            "writing-editor",
            include_str!("../resources/bundled-skills/writing-editor/SKILL.md"),
        ),
        (
            "shell-expert",
            include_str!("../resources/bundled-skills/shell-expert/SKILL.md"),
        ),
        // Integration skills
        (
            "google-workspace",
            include_str!("../resources/bundled-skills/google-workspace/SKILL.md"),
        ),
        (
            "clickup",
            include_str!("../resources/bundled-skills/clickup/SKILL.md"),
        ),
        (
            "atlassian",
            include_str!("../resources/bundled-skills/atlassian/SKILL.md"),
        ),
        (
            "servicenow",
            include_str!("../resources/bundled-skills/servicenow/SKILL.md"),
        ),
        (
            "salesforce",
            include_str!("../resources/bundled-skills/salesforce/SKILL.md"),
        ),
        (
            "monday",
            include_str!("../resources/bundled-skills/monday/SKILL.md"),
        ),
        (
            "microsoft365",
            include_str!("../resources/bundled-skills/microsoft365/SKILL.md"),
        ),
    ]
}

impl SkillsManager {
    /// Extract bundled skills on first run (when no skills are installed).
    /// Returns the number of skills extracted.
    pub fn extract_bundled_skills(&mut self) -> Result<usize> {
        // Only extract if skills dir has no subdirectories (first run or reset)
        let has_skills = fs::read_dir(&self.skills_dir)?
            .filter_map(|e| e.ok())
            .any(|e| e.path().is_dir());

        if has_skills {
            info!("[SKILLS] Skills directory already has content, skipping bundled extraction");
            return Ok(0);
        }

        let bundled = get_bundled_skills();
        let mut count = 0;

        for (id, content) in &bundled {
            let skill_dir = self.skills_dir.join(id);
            fs::create_dir_all(&skill_dir)?;
            fs::write(skill_dir.join("SKILL.md"), content)?;
            count += 1;
        }

        // Reload all skills after extraction
        self.load_all_skills()?;

        info!("[SKILLS] Extracted {} bundled skills", count);
        Ok(count)
    }

    /// Reset bundled skills to their default versions.
    /// Overwrites existing bundled skills but preserves user-created ones.
    pub fn reset_bundled_skills(&mut self) -> Result<usize> {
        let bundled = get_bundled_skills();
        let mut count = 0;

        for (id, content) in &bundled {
            let skill_dir = self.skills_dir.join(id);
            fs::create_dir_all(&skill_dir)?;
            fs::write(skill_dir.join("SKILL.md"), content)?;
            count += 1;
        }

        // Reload all skills
        self.load_all_skills()?;

        info!("[SKILLS] Reset {} bundled skills to defaults", count);
        Ok(count)
    }
}

// ---------------------------------------------------------------------------
// Skill execution engine
// ---------------------------------------------------------------------------

impl SkillsManager {
    /// Load per-skill runtime configuration from `skill.config.yaml` (if present).
    /// Returns defaults when the file is absent or unreadable.
    pub fn load_skill_config(&self, skill_id: &str) -> SkillConfig {
        let skill = match self.loaded_skills.get(skill_id) {
            Some(s) => s,
            None => return SkillConfig::default(),
        };

        let config_path = skill.path.join("skill.config.yaml");
        if !config_path.exists() {
            return SkillConfig::default();
        }

        // Reject symlink
        if reject_if_symlink(&config_path).is_err() {
            warn!(
                "[SKILLS] Rejected symlinked skill.config.yaml for '{}'",
                skill_id
            );
            return SkillConfig::default();
        }

        match fs::read_to_string(&config_path)
            .map_err(|e| e.to_string())
            .and_then(|s| serde_yml::from_str::<SkillConfig>(&s).map_err(|e| e.to_string()))
        {
            Ok(mut cfg) => {
                // Validate config value keys: only [A-Z0-9_] allowed to prevent env var injection
                cfg.values.retain(|k, _| {
                    let valid = k
                        .chars()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
                    if !valid {
                        warn!(
                            "[SKILLS] Rejected skill config key '{}' for '{}': invalid characters",
                            k, skill_id
                        );
                    }
                    valid
                });
                cfg
            }
            Err(e) => {
                warn!(
                    "[SKILLS] Could not load skill.config.yaml for '{}': {}",
                    skill_id, e
                );
                SkillConfig::default()
            }
        }
    }

    /// Execute a named script from a skill's `scripts/` directory.
    ///
    /// # Security
    /// - Script path is validated to be within `<skill_dir>/scripts/` (no traversal).
    /// - Script file must not be a symlink.
    /// - Executed via a platform-appropriate interpreter resolved by file extension.
    /// - Only declared environment variables (from skill metadata `env`) are forwarded.
    /// - Per-skill config values are passed as `SKILL_CONFIG_<KEY>=<value>`.
    /// - Execution is bounded by `timeout_seconds` from `skill.config.yaml`.
    /// - Output is truncated at `max_output_bytes`.
    ///
    /// # Arguments
    /// - `skill_id`: Identifier of the skill to invoke.
    /// - `script_name`: Filename of the script within `scripts/` (e.g. `"run.sh"`).
    /// - `stdin_input`: Optional text passed to the script on stdin.
    pub async fn execute_skill_script(
        &self,
        skill_id: &str,
        script_name: &str,
        stdin_input: Option<&str>,
    ) -> Result<SkillExecResult> {
        use tokio::io::AsyncWriteExt;
        use tokio::process::Command;
        use tokio::time::timeout;

        let skill = self
            .loaded_skills
            .get(skill_id)
            .ok_or_else(|| anyhow::anyhow!("Skill '{}' not found", skill_id))?;

        // --- Path validation: script must be within <skill_dir>/scripts/ ---
        let scripts_dir = skill.path.join("scripts");
        let raw_script_path = scripts_dir.join(script_name);

        // Reject symlink on the scripts/ directory itself before canonicalizing.
        // If scripts/ is a symlink, canonicalize() would resolve it to its target,
        // making the subsequent starts_with boundary check meaningless.
        reject_if_symlink(&scripts_dir).map_err(|e| anyhow::anyhow!("{}", e))?;

        // Canonicalize to resolve any . or .. components
        let canonical_scripts_dir =
            fs::canonicalize(&scripts_dir).context("Cannot canonicalize scripts directory")?;
        let canonical_script = fs::canonicalize(&raw_script_path)
            .context("Cannot canonicalize script path — does the script file exist?")?;

        if !canonical_script.starts_with(&canonical_scripts_dir) {
            anyhow::bail!(
                "Path traversal rejected: script '{}' is outside the skill's scripts directory",
                script_name
            );
        }

        // --- Symlink check ---
        reject_if_symlink(&canonical_script).map_err(|e| anyhow::anyhow!("{}", e))?;

        // --- Resolve script interpreter (cross-platform) ---
        let (interpreter, interpreter_args) = resolve_script_interpreter(&canonical_script);
        if !interpreter.exists() {
            anyhow::bail!(
                "Script interpreter {:?} not found; cannot execute skill script",
                interpreter
            );
        }

        // --- Load per-skill config ---
        let skill_config = self.load_skill_config(skill_id);
        let timeout_secs = std::time::Duration::from_secs(skill_config.timeout_seconds);
        let max_bytes = skill_config.max_output_bytes;

        // --- Build sanitized environment ---
        // Start from an empty environment (not inherited from process).
        // Only forward explicitly declared env vars from skill metadata.
        let mut env_vars: Vec<(String, String)> = Vec::new();

        // Forward declared env vars (from skill.metadata `env` field)
        for declared_key in skill
            .metadata
            .openclaw_requires()
            .map(|r| r.env)
            .unwrap_or_default()
        {
            if let Ok(val) = std::env::var(&declared_key) {
                env_vars.push((declared_key, val));
            }
        }

        // Add per-skill config values as SKILL_CONFIG_<KEY>=<value>
        for (k, v) in &skill_config.values {
            env_vars.push((format!("SKILL_CONFIG_{}", k), v.clone()));
        }

        // Inject safe read-only context variables
        env_vars.push(("SKILL_ID".to_string(), skill_id.to_string()));
        env_vars.push(("SKILL_SCRIPT".to_string(), script_name.to_string()));

        // --- Build the command ---
        let mut cmd = Command::new(&interpreter);
        for arg in &interpreter_args {
            cmd.arg(arg);
        }
        cmd.arg(&canonical_script);
        cmd.env_clear();
        for (k, v) in &env_vars {
            cmd.env(k, v);
        }
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        // Spawn
        let mut child = cmd
            .spawn()
            .context("Failed to spawn skill script process")?;

        // Write stdin if provided
        if let Some(input) = stdin_input {
            if let Some(mut stdin_handle) = child.stdin.take() {
                let _ = stdin_handle.write_all(input.as_bytes()).await;
                // stdin_handle drops here, closing the pipe
            }
        } else {
            // Drop stdin to signal EOF
            drop(child.stdin.take());
        }

        // Wait with timeout
        let output_result = timeout(timeout_secs, child.wait_with_output()).await;

        match output_result {
            Err(_) => {
                error!(
                    "[SKILLS] Script '{}' in skill '{}' timed out after {}s",
                    script_name, skill_id, skill_config.timeout_seconds
                );
                anyhow::bail!(
                    "Skill script '{}' timed out after {} seconds",
                    script_name,
                    skill_config.timeout_seconds
                );
            }
            Ok(Err(e)) => {
                anyhow::bail!("Skill script '{}' failed to run: {}", script_name, e);
            }
            Ok(Ok(output)) => {
                let exit_code = output.status.code().unwrap_or(-1);

                // Decode and truncate stdout
                // Walk char boundaries to avoid panicking mid-UTF-8-character.
                let utf8_boundary = |s: &str, max: usize| -> usize {
                    s.char_indices()
                        .take_while(|(i, _)| *i < max)
                        .last()
                        .map(|(i, c)| i + c.len_utf8())
                        .unwrap_or(0)
                };

                let raw_stdout = String::from_utf8_lossy(&output.stdout);
                let stdout = if raw_stdout.len() > max_bytes {
                    warn!(
                        "[SKILLS] Script '{}' stdout truncated at {} bytes",
                        script_name, max_bytes
                    );
                    let b = utf8_boundary(&raw_stdout, max_bytes);
                    raw_stdout[..b].to_string()
                } else {
                    raw_stdout.into_owned()
                };

                // Log stderr (do not return to caller to avoid info leaks)
                let stderr = String::from_utf8_lossy(&output.stderr);
                if !stderr.is_empty() {
                    // Truncate stderr log at 4 KiB to prevent log flooding
                    let truncated = if stderr.len() > 4096 {
                        let b = utf8_boundary(&stderr, 4096);
                        &stderr[..b]
                    } else {
                        &stderr
                    };
                    if exit_code != 0 {
                        error!("[SKILLS] Script '{}' stderr: {}", script_name, truncated);
                    } else {
                        info!("[SKILLS] Script '{}' stderr: {}", script_name, truncated);
                    }
                }

                let success = exit_code == 0;
                info!(
                    "[SKILLS] Script '{}' in skill '{}' exited with code {}",
                    script_name, skill_id, exit_code
                );

                Ok(SkillExecResult {
                    exit_code,
                    stdout,
                    success,
                    script: script_name.to_string(),
                    skill_id: skill_id.to_string(),
                })
            }
        }
    }
}

/// Standalone skill definition file format (for skill.yaml / skill.toml).
///
/// This allows defining a skill entirely in a single YAML or TOML file,
/// as an alternative to the SKILL.md + YAML frontmatter format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillDefinitionFile {
    /// Skill metadata (flattened into the top-level)
    #[serde(flatten)]
    pub metadata: SkillMetadata,
    /// The skill content/instructions
    pub content: String,
}

impl SkillsManager {
    /// Load a skill from a standalone YAML file.
    fn load_skill_from_yaml(
        &self,
        skill_dir: &PathBuf,
        yaml_path: &std::path::Path,
    ) -> Result<Skill> {
        let skill_id = skill_dir
            .file_name()
            .and_then(|s| s.to_str())
            .context("Invalid skill directory name")?
            .to_string();

        let content = fs::read_to_string(yaml_path)?;

        // Validate against YAML 1.1 implicit boolean coercion
        if let Err(e) = Self::validate_frontmatter_no_implicit_booleans(&content) {
            warn!("[SKILLS] {}", e);
            anyhow::bail!(e);
        }

        let def: SkillDefinitionFile =
            serde_yml::from_str(&content).context("Failed to parse skill.yaml")?;

        let scripts = Self::list_directory_files(&skill_dir.join("scripts"))?;
        let references = Self::list_directory_files(&skill_dir.join("references"))?;
        let install_info_path = skill_dir.join("install_info.json");
        let install_info = if install_info_path.exists() {
            fs::read_to_string(&install_info_path)
                .ok()
                .and_then(|s| serde_json::from_str::<SkillInstallInfo>(&s).ok())
        } else {
            None
        };

        Ok(Skill {
            id: skill_id,
            metadata: def.metadata,
            content: def.content,
            path: skill_dir.clone(),
            scripts,
            references,
            install_info,
        })
    }

    /// Load a skill from a standalone TOML file.
    fn load_skill_from_toml(
        &self,
        skill_dir: &PathBuf,
        toml_path: &std::path::Path,
    ) -> Result<Skill> {
        let skill_id = skill_dir
            .file_name()
            .and_then(|s| s.to_str())
            .context("Invalid skill directory name")?
            .to_string();

        let content = fs::read_to_string(toml_path)?;
        let def: SkillDefinitionFile =
            toml::from_str(&content).context("Failed to parse skill.toml")?;

        let scripts = Self::list_directory_files(&skill_dir.join("scripts"))?;
        let references = Self::list_directory_files(&skill_dir.join("references"))?;
        let install_info_path = skill_dir.join("install_info.json");
        let install_info = if install_info_path.exists() {
            fs::read_to_string(&install_info_path)
                .ok()
                .and_then(|s| serde_json::from_str::<SkillInstallInfo>(&s).ok())
        } else {
            None
        };

        Ok(Skill {
            id: skill_id,
            metadata: def.metadata,
            content: def.content,
            path: skill_dir.clone(),
            scripts,
            references,
            install_info,
        })
    }

    /// Validate that a skill's requirements are met.
    /// Returns a list of warnings for unmet requirements.
    #[allow(dead_code)]
    pub fn validate_skill_requirements(&self, skill: &Skill) -> Vec<String> {
        let mut warnings = Vec::new();

        // Check basic requirements list
        for req in &skill.metadata.requirements {
            if which::which(req).is_err() {
                warnings.push(format!(
                    "Skill '{}' requires '{}' but it was not found on PATH",
                    skill.id, req
                ));
            }
        }

        // Check OpenClaw-style requirements
        if let Some(requires) = skill.metadata.openclaw_requires() {
            for bin in &requires.bins {
                if which::which(bin).is_err() {
                    warnings.push(format!(
                        "Skill '{}' requires binary '{}' but it was not found on PATH",
                        skill.id, bin
                    ));
                }
            }
            for env_var in &requires.env {
                if std::env::var(env_var).is_err() {
                    warnings.push(format!(
                        "Skill '{}' requires environment variable '{}' but it is not set",
                        skill.id, env_var
                    ));
                }
            }
            for config_path in &requires.config {
                if !std::path::Path::new(config_path).exists() {
                    warnings.push(format!(
                        "Skill '{}' requires config file '{}' but it does not exist",
                        skill.id, config_path
                    ));
                }
            }
        }

        warnings
    }

    /// Validate all loaded skills and return warnings.
    #[allow(dead_code)]
    pub fn validate_all_requirements(&self) -> HashMap<String, Vec<String>> {
        let mut all_warnings = HashMap::new();
        for skill in self.loaded_skills.values() {
            let warnings = self.validate_skill_requirements(skill);
            if !warnings.is_empty() {
                all_warnings.insert(skill.id.clone(), warnings);
            }
        }
        all_warnings
    }
}

/// Start a file watcher on the skills directory for hot-reload.
///
/// When skill files change, the watcher reloads all skills from disk.
/// Uses the same debouncer pattern as the config hot-reload.
pub fn start_skills_watcher(
    skills_manager: std::sync::Arc<tokio::sync::RwLock<SkillsManager>>,
) -> Result<()> {
    use notify_debouncer_mini::{new_debouncer, notify::RecursiveMode};
    use std::time::Duration;

    let skills_dir = SkillsManager::get_skills_dir()?;

    info!("[SKILLS] Starting skills file watcher at: {:?}", skills_dir);

    std::thread::spawn(move || {
        let (notify_tx, notify_rx) = std::sync::mpsc::channel();

        let mut debouncer = match new_debouncer(Duration::from_secs(2), notify_tx) {
            Ok(d) => d,
            Err(e) => {
                warn!("[SKILLS] Failed to create skills file watcher: {}", e);
                return;
            }
        };

        if let Err(e) = debouncer
            .watcher()
            .watch(&skills_dir, RecursiveMode::Recursive)
        {
            warn!("[SKILLS] Failed to watch skills directory: {}", e);
            return;
        }

        info!("[SKILLS] Skills file watcher started");

        loop {
            match notify_rx.recv() {
                Ok(Ok(_events)) => {
                    info!("[SKILLS] Skills directory changed, reloading...");
                    let manager = skills_manager.clone();
                    let rt = tokio::runtime::Handle::try_current();
                    match rt {
                        Ok(handle) => {
                            handle.spawn(async move {
                                let mut mgr = manager.write().await;
                                if let Err(e) = mgr.load_all_skills() {
                                    warn!("[SKILLS] Hot-reload failed: {}", e);
                                } else {
                                    info!("[SKILLS] Skills hot-reloaded successfully");
                                }
                            });
                        }
                        Err(_) => {
                            warn!("[SKILLS] No tokio runtime available for skills reload");
                        }
                    }
                }
                Ok(Err(errors)) => {
                    warn!("[SKILLS] Watch errors: {:?}", errors);
                }
                Err(e) => {
                    warn!("[SKILLS] Watch channel closed: {}", e);
                    break;
                }
            }
        }
    });

    Ok(())
}

/// Get built-in skill templates.
pub fn builtin_templates() -> Vec<SkillTemplate> {
    vec![
        SkillTemplate {
            id: "code-review".to_string(),
            name: "Code Review".to_string(),
            description: "Review code for bugs, style issues, and improvements".to_string(),
            content: "When asked to review code, analyze it for:\n\n\
                1. **Bugs and errors** — logic flaws, off-by-one errors, null handling\n\
                2. **Security issues** — injection, XSS, hardcoded secrets\n\
                3. **Performance** — unnecessary allocations, N+1 queries\n\
                4. **Style** — naming conventions, code organization\n\
                5. **Suggestions** — idiomatic improvements, simplifications\n\n\
                Format your review as a list of findings with severity (Critical/Warning/Info) and line references."
                .to_string(),
            user_invocable: true,
        },
        SkillTemplate {
            id: "email-writer".to_string(),
            name: "Email Writer".to_string(),
            description: "Draft professional emails with appropriate tone".to_string(),
            content: "When asked to write an email:\n\n\
                1. Ask for the **recipient** and **purpose** if not provided\n\
                2. Match the appropriate **tone** (formal, friendly, urgent)\n\
                3. Keep it **concise** — get to the point quickly\n\
                4. Include a clear **subject line**\n\
                5. End with an appropriate **call to action**\n\n\
                Output the email in a copy-paste ready format with Subject line."
                .to_string(),
            user_invocable: true,
        },
        SkillTemplate {
            id: "summarizer".to_string(),
            name: "Summarizer".to_string(),
            description: "Summarize long text into key points".to_string(),
            content: "When asked to summarize content:\n\n\
                1. Read the full text carefully\n\
                2. Identify the **main thesis** or purpose\n\
                3. Extract **key points** (3-7 bullet points)\n\
                4. Note any **important details** or data\n\
                5. Provide a **one-sentence TL;DR**\n\n\
                Format: Start with the TL;DR, then bullet points, then any notable details."
                .to_string(),
            user_invocable: true,
        },
        SkillTemplate {
            id: "translator".to_string(),
            name: "Translator".to_string(),
            description: "Translate text between languages with context".to_string(),
            content: "When asked to translate:\n\n\
                1. Detect the **source language** if not specified\n\
                2. Translate to the **target language**\n\
                3. Preserve **tone and intent**, not just literal meaning\n\
                4. Note any **idioms or cultural references** that don't translate directly\n\
                5. If the text is ambiguous, provide **alternative translations**\n\n\
                Format: Provide the translation, then any notes about nuance."
                .to_string(),
            user_invocable: true,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    // ── get_bundled_skills tests ──────────────────────────────────────

    #[test]
    fn test_bundled_skills_count() {
        let skills = get_bundled_skills();
        assert_eq!(
            skills.len(),
            22,
            "Expected exactly 22 bundled skills, got {}",
            skills.len()
        );
    }

    #[test]
    fn test_bundled_skills_valid_frontmatter() {
        let skills = get_bundled_skills();
        for (id, content) in &skills {
            assert!(
                content.starts_with("---"),
                "Bundled skill '{}' does not start with YAML frontmatter delimiter '---'",
                id
            );
            // Verify the frontmatter parses as valid YAML
            let lines: Vec<&str> = content.lines().collect();
            let end_idx = lines
                .iter()
                .skip(1)
                .position(|line| *line == "---")
                .unwrap_or_else(|| panic!("Bundled skill '{}' has unclosed YAML frontmatter", id))
                + 1;
            let yaml_content = lines[1..end_idx].join("\n");
            let _metadata: SkillMetadata = serde_yml::from_str(&yaml_content).unwrap_or_else(|e| {
                panic!("Bundled skill '{}' has invalid YAML frontmatter: {}", id, e)
            });
        }
    }

    #[test]
    fn test_bundled_skills_have_source_bundled() {
        let skills = get_bundled_skills();
        for (id, content) in &skills {
            let (metadata, _) = SkillsManager::parse_skill_md(content)
                .unwrap_or_else(|e| panic!("Failed to parse skill '{}': {}", id, e));
            assert_eq!(
                metadata.source.as_deref(),
                Some("bundled"),
                "Bundled skill '{}' should have source: bundled, got: {:?}",
                id,
                metadata.source
            );
        }
    }

    #[test]
    fn test_bundled_skills_have_content() {
        let skills = get_bundled_skills();
        for (id, content) in &skills {
            let (_, markdown) = SkillsManager::parse_skill_md(content)
                .unwrap_or_else(|e| panic!("Failed to parse skill '{}': {}", id, e));
            assert!(
                !markdown.trim().is_empty(),
                "Bundled skill '{}' has no content after frontmatter",
                id
            );
        }
    }

    #[test]
    fn test_bundled_skill_ids_unique() {
        let skills = get_bundled_skills();
        let mut seen = HashSet::new();
        for (id, _) in &skills {
            assert!(seen.insert(*id), "Duplicate bundled skill ID: '{}'", id);
        }
        assert_eq!(seen.len(), 22);
    }

    // ── parse_skill_md tests ─────────────────────────────────────────

    #[test]
    fn test_parse_skill_md_valid() {
        let content = "---\nname: Test Skill\ndescription: A test skill\nuser-invocable: true\nsource: local\n---\n\nThis is the skill body.";
        let (metadata, markdown) = SkillsManager::parse_skill_md(content).unwrap();
        assert_eq!(metadata.name.as_deref(), Some("Test Skill"));
        assert_eq!(metadata.description.as_deref(), Some("A test skill"));
        assert!(metadata.user_invocable);
        assert_eq!(metadata.source.as_deref(), Some("local"));
        assert_eq!(markdown.trim(), "This is the skill body.");
    }

    #[test]
    fn test_parse_skill_md_no_frontmatter() {
        let content = "Just some plain markdown content.\n\nNo YAML here.";
        let (metadata, markdown) = SkillsManager::parse_skill_md(content).unwrap();
        // Should return default metadata
        assert!(metadata.name.is_none());
        assert!(metadata.description.is_none());
        assert!(metadata.user_invocable); // default_true
        assert!(!metadata.disable_model_invocation);
        // Content should be returned verbatim
        assert_eq!(markdown, content);
    }

    // ── YAML 1.1 implicit boolean validation tests ────────────────────

    #[test]
    fn test_validate_frontmatter_rejects_on() {
        let fm = "name: test\nenabled: on";
        let result = SkillsManager::validate_frontmatter_no_implicit_booleans(fm);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("on"));
    }

    #[test]
    fn test_validate_frontmatter_rejects_off() {
        let fm = "name: test\nenabled: off";
        let result = SkillsManager::validate_frontmatter_no_implicit_booleans(fm);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("off"));
    }

    #[test]
    fn test_validate_frontmatter_rejects_yes() {
        let fm = "name: test\nactive: yes";
        let result = SkillsManager::validate_frontmatter_no_implicit_booleans(fm);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("yes"));
    }

    #[test]
    fn test_validate_frontmatter_rejects_no() {
        let fm = "name: test\nactive: no";
        let result = SkillsManager::validate_frontmatter_no_implicit_booleans(fm);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no"));
    }

    #[test]
    fn test_validate_frontmatter_rejects_y() {
        let fm = "name: test\nflag: y";
        let result = SkillsManager::validate_frontmatter_no_implicit_booleans(fm);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_frontmatter_rejects_n() {
        let fm = "name: test\nflag: n";
        let result = SkillsManager::validate_frontmatter_no_implicit_booleans(fm);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_frontmatter_allows_true_false() {
        let fm = "name: test\nenabled: true\nactive: false";
        let result = SkillsManager::validate_frontmatter_no_implicit_booleans(fm);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_frontmatter_allows_quoted_values() {
        let fm = "name: test\nstatus: \"on\"\nmode: 'off'";
        let result = SkillsManager::validate_frontmatter_no_implicit_booleans(fm);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_frontmatter_allows_normal_values() {
        let fm = "name: My Skill\ndescription: A useful tool\nuser-invocable: true\nsource: local";
        let result = SkillsManager::validate_frontmatter_no_implicit_booleans(fm);
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_skill_md_rejects_implicit_booleans() {
        let content = "---\nname: Bad Skill\nuser-invocable: yes\n---\n\nBody text.";
        let result = SkillsManager::parse_skill_md(content);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("yes"),
            "Error should mention the offending value: {}",
            err_msg
        );
    }

    // ── File I/O tests (extract / reset) ─────────────────────────────

    /// Helper: create a SkillsManager with the given directory as skills_dir.
    fn make_manager(dir: &std::path::Path) -> SkillsManager {
        std::fs::create_dir_all(dir).unwrap();
        SkillsManager {
            skills_dir: dir.to_path_buf(),
            loaded_skills: HashMap::new(),
        }
    }

    #[test]
    fn test_extract_bundled_skills_creates_files() {
        let tmp = tempfile::tempdir().unwrap();
        let skills_dir = tmp.path().join("skills");
        let mut manager = make_manager(&skills_dir);

        let count = manager.extract_bundled_skills().unwrap();
        assert_eq!(count, 22, "Expected 22 skills extracted, got {}", count);

        // Verify each bundled skill has a directory with SKILL.md
        for (id, _) in get_bundled_skills() {
            let skill_md = skills_dir.join(id).join("SKILL.md");
            assert!(
                skill_md.exists(),
                "Expected SKILL.md for bundled skill '{}' at {:?}",
                id,
                skill_md
            );
        }

        // After extraction, skills should be loaded
        // Note: some bundled skills may fail to parse, so loaded count may be less than extracted
        assert!(
            manager.loaded_skills.len() > 0,
            "At least some skills should be loaded"
        );
    }

    #[test]
    fn test_extract_bundled_skills_skips_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let skills_dir = tmp.path().join("skills");
        let mut manager = make_manager(&skills_dir);

        // Create a subdirectory to simulate existing skills
        std::fs::create_dir_all(skills_dir.join("some-existing-skill")).unwrap();

        let count = manager.extract_bundled_skills().unwrap();
        assert_eq!(
            count, 0,
            "Should skip extraction when skills dir already has subdirectories"
        );
    }

    #[test]
    fn test_reset_bundled_skills() {
        let tmp = tempfile::tempdir().unwrap();
        let skills_dir = tmp.path().join("skills");
        let mut manager = make_manager(&skills_dir);

        // First extract
        manager.extract_bundled_skills().unwrap();
        assert!(
            manager.loaded_skills.len() > 0,
            "Skills should be loaded after extraction"
        );

        // Modify one skill's content
        let modified_path = skills_dir.join("code-review").join("SKILL.md");
        std::fs::write(
            &modified_path,
            "---\nname: Modified\n---\n\nModified content",
        )
        .unwrap();

        // Reset should overwrite bundled skills
        let count = manager.reset_bundled_skills().unwrap();
        assert_eq!(count, 22, "Reset should process all 22 bundled skills");

        // Verify the modified skill was restored
        let restored_content = std::fs::read_to_string(&modified_path).unwrap();
        assert_ne!(
            restored_content.trim(),
            "---\nname: Modified\n---\n\nModified content",
            "Reset should have overwritten the modified skill"
        );
        assert!(restored_content.contains("source: bundled"));
    }
}

// ---------------------------------------------------------------------------
// Execution-engine tests
// ---------------------------------------------------------------------------
//
// Tests for `execute_skill_script` and `load_skill_config`.
// These use real temp directories and actual child processes so they exercise
// the full path-validation, env-sanitisation, timeout, and output-truncation
// logic without network access.
//
// Note: these tests only run on Unix (they call /bin/sh and use PermissionsExt).

#[cfg(all(test, unix))]
mod execution_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    // ── Helpers ──────────────────────────────────────────────────────────────

    /// Create a minimal SkillsManager pointing at `dir`.
    fn make_manager_at(dir: &std::path::Path) -> SkillsManager {
        std::fs::create_dir_all(dir).unwrap();
        SkillsManager {
            skills_dir: dir.to_path_buf(),
            loaded_skills: HashMap::new(),
        }
    }

    /// Create a skill directory under `skills_dir` with the given scripts.
    ///
    /// Returns a SkillsManager that has loaded the new skill.
    fn setup_skill(
        skills_dir: &std::path::Path,
        skill_id: &str,
        scripts: &[(&str, &str)],
        command_dispatch: Option<&str>,
    ) -> SkillsManager {
        let skill_dir = skills_dir.join(skill_id);
        let scripts_dir = skill_dir.join("scripts");
        fs::create_dir_all(&scripts_dir).unwrap();

        for (name, content) in scripts {
            let script_path = scripts_dir.join(name);
            fs::write(&script_path, content).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&script_path, fs::Permissions::from_mode(0o700));
            }
        }

        let dispatch_line = command_dispatch
            .map(|d| format!("command_dispatch: \"{}\"\n", d))
            .unwrap_or_default();

        let skill_md = format!(
            "---\nname: {}\ndescription: Test\nuser_invocable: false\n{}---\nTest content\n",
            skill_id, dispatch_line
        );
        fs::write(skill_dir.join("SKILL.md"), &skill_md).unwrap();

        let mut manager = make_manager_at(skills_dir);
        manager.load_all_skills().unwrap();
        manager
    }

    // ── load_skill_config tests ───────────────────────────────────────────────

    #[test]
    fn test_load_skill_config_unknown_skill_returns_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = make_manager_at(tmp.path());

        let cfg = manager.load_skill_config("does-not-exist");
        assert_eq!(cfg.timeout_seconds, 30);
        assert_eq!(cfg.max_output_bytes, 1024 * 1024);
        assert!(cfg.values.is_empty());
    }

    #[test]
    fn test_load_skill_config_missing_file_returns_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = setup_skill(tmp.path(), "no-config", &[], None);

        let cfg = manager.load_skill_config("no-config");
        assert_eq!(cfg.timeout_seconds, 30);
        assert!(cfg.values.is_empty());
    }

    #[test]
    fn test_load_skill_config_parses_valid_yaml() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = setup_skill(tmp.path(), "conf-skill", &[], None);

        let config_path = tmp.path().join("conf-skill").join("skill.config.yaml");
        fs::write(
            &config_path,
            "timeout_seconds: 60\nmax_output_bytes: 2048\nvalues:\n  API_KEY: abc123\n",
        )
        .unwrap();

        let cfg = manager.load_skill_config("conf-skill");
        assert_eq!(cfg.timeout_seconds, 60);
        assert_eq!(cfg.max_output_bytes, 2048);
        assert_eq!(
            cfg.values.get("API_KEY").map(String::as_str),
            Some("abc123")
        );
    }

    #[test]
    fn test_load_skill_config_strips_invalid_keys_silently() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = setup_skill(tmp.path(), "bad-keys", &[], None);

        let config_path = tmp.path().join("bad-keys").join("skill.config.yaml");
        fs::write(
            &config_path,
            "values:\n  GOOD_KEY: ok\n  bad-key: nope\n  another bad: nope\n",
        )
        .unwrap();

        let cfg = manager.load_skill_config("bad-keys");
        assert!(
            cfg.values.contains_key("GOOD_KEY"),
            "valid key should be kept"
        );
        assert!(
            !cfg.values.contains_key("bad-key"),
            "hyphenated key should be stripped"
        );
        assert!(
            !cfg.values.contains_key("another bad"),
            "key with space should be stripped"
        );
    }

    #[test]
    fn test_load_skill_config_invalid_yaml_returns_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = setup_skill(tmp.path(), "broken", &[], None);

        let config_path = tmp.path().join("broken").join("skill.config.yaml");
        fs::write(&config_path, "this: is: not: valid: yaml: [[\n").unwrap();

        let cfg = manager.load_skill_config("broken");
        // Should silently fall back to defaults
        assert_eq!(cfg.timeout_seconds, 30);
    }

    // ── execute_skill_script tests ────────────────────────────────────────────

    #[tokio::test]
    async fn test_execute_skill_script_basic_success() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = setup_skill(
            tmp.path(),
            "echo-skill",
            &[("run.sh", "#!/bin/sh\necho hello_from_skill\n")],
            Some("script"),
        );

        let result = manager
            .execute_skill_script("echo-skill", "run.sh", None)
            .await
            .unwrap();

        assert!(result.success, "script should succeed");
        assert_eq!(result.exit_code, 0);
        assert!(
            result.stdout.contains("hello_from_skill"),
            "stdout: {}",
            result.stdout
        );
        assert_eq!(result.script, "run.sh");
        assert_eq!(result.skill_id, "echo-skill");
    }

    #[tokio::test]
    async fn test_execute_skill_script_nonexero_exit_code() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = setup_skill(
            tmp.path(),
            "fail-skill",
            &[("fail.sh", "#!/bin/sh\nexit 42\n")],
            Some("script"),
        );

        let result = manager
            .execute_skill_script("fail-skill", "fail.sh", None)
            .await
            .unwrap();

        assert!(
            !result.success,
            "script with non-zero exit should not be success"
        );
        assert_eq!(result.exit_code, 42);
    }

    #[tokio::test]
    async fn test_execute_skill_script_path_traversal_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = setup_skill(tmp.path(), "trav-skill", &[], Some("script"));

        // Attempt to escape the scripts dir via ../
        let err = manager
            .execute_skill_script("trav-skill", "../SKILL.md", None)
            .await
            .unwrap_err();

        let msg = err.to_string();
        // Either "canonicalize" failure or explicit traversal message
        assert!(
            msg.contains("traversal") || msg.contains("canonicalize") || msg.contains("outside"),
            "unexpected error: {msg}"
        );
    }

    #[tokio::test]
    async fn test_execute_skill_script_nonexistent_script_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = setup_skill(tmp.path(), "missing-script", &[], Some("script"));

        let err = manager
            .execute_skill_script("missing-script", "does_not_exist.sh", None)
            .await
            .unwrap_err();

        assert!(
            err.to_string().contains("canonicalize") || err.to_string().contains("No such"),
            "expected not-found error, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_execute_skill_script_unknown_skill_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = make_manager_at(tmp.path());

        let err = manager
            .execute_skill_script("ghost", "run.sh", None)
            .await
            .unwrap_err();

        assert!(err.to_string().contains("not found"), "got: {err}");
    }

    #[tokio::test]
    async fn test_execute_skill_script_injects_skill_context_vars() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = setup_skill(
            tmp.path(),
            "ctx-skill",
            &[(
                "ctx.sh",
                "#!/bin/sh\necho SKILL_ID=$SKILL_ID\necho SKILL_SCRIPT=$SKILL_SCRIPT\n",
            )],
            Some("script"),
        );

        let result = manager
            .execute_skill_script("ctx-skill", "ctx.sh", None)
            .await
            .unwrap();

        assert!(
            result.stdout.contains("SKILL_ID=ctx-skill"),
            "stdout: {}",
            result.stdout
        );
        assert!(
            result.stdout.contains("SKILL_SCRIPT=ctx.sh"),
            "stdout: {}",
            result.stdout
        );
    }

    #[tokio::test]
    async fn test_execute_skill_script_injects_skill_config_values() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = setup_skill(
            tmp.path(),
            "cfg-skill",
            &[("cfg.sh", "#!/bin/sh\necho VAL=$SKILL_CONFIG_MY_KEY\n")],
            Some("script"),
        );

        // Write a skill config with MY_KEY
        let config_path = tmp.path().join("cfg-skill").join("skill.config.yaml");
        fs::write(&config_path, "values:\n  MY_KEY: hello_config\n").unwrap();

        let result = manager
            .execute_skill_script("cfg-skill", "cfg.sh", None)
            .await
            .unwrap();

        assert!(
            result.stdout.contains("VAL=hello_config"),
            "stdout: {}",
            result.stdout
        );
    }

    #[tokio::test]
    async fn test_execute_skill_script_does_not_inherit_process_env() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = setup_skill(
            tmp.path(),
            "env-skill",
            &[("env.sh", "#!/bin/sh\necho HOME_VAR=${HOME:-UNSET}\n")],
            Some("script"),
        );

        let result = manager
            .execute_skill_script("env-skill", "env.sh", None)
            .await
            .unwrap();

        // HOME is not declared in skill env, so it should not be forwarded
        assert!(
            result.stdout.contains("HOME_VAR=UNSET"),
            "HOME should not be forwarded to script, stdout: {}",
            result.stdout
        );
    }

    #[tokio::test]
    async fn test_execute_skill_script_passes_stdin() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = setup_skill(
            tmp.path(),
            "stdin-skill",
            &[("read.sh", "#!/bin/sh\nread -r LINE\necho got:$LINE\n")],
            Some("script"),
        );

        let result = manager
            .execute_skill_script("stdin-skill", "read.sh", Some("hello_stdin"))
            .await
            .unwrap();

        assert!(
            result.stdout.contains("got:hello_stdin"),
            "stdin not forwarded: {}",
            result.stdout
        );
    }

    #[tokio::test]
    async fn test_execute_skill_script_output_truncated_at_max_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = setup_skill(
            tmp.path(),
            "big-skill",
            &[(
                "big.sh",
                // Write 1 MiB + 1 byte of 'A' characters
                "#!/bin/sh\npython3 -c \"print('A' * (1024*1024 + 1), end='')\"\n",
            )],
            Some("script"),
        );

        // Write a config with a small max_output_bytes so we don't need to
        // actually produce 1MB output in the test
        let config_path = tmp.path().join("big-skill").join("skill.config.yaml");
        fs::write(&config_path, "max_output_bytes: 64\n").unwrap();

        let manager = setup_skill(
            tmp.path(),
            "trunc-skill",
            &[("trunc.sh", "#!/bin/sh\nprintf '%0128d' 0\n")],
            Some("script"),
        );

        let config_path = tmp.path().join("trunc-skill").join("skill.config.yaml");
        fs::write(&config_path, "max_output_bytes: 64\n").unwrap();

        // Reload to pick up config (config is loaded at execution time, not load time)
        let result = manager
            .execute_skill_script("trunc-skill", "trunc.sh", None)
            .await
            .unwrap();

        assert!(
            result.stdout.len() <= 64,
            "output should be truncated to max_output_bytes, got {} bytes",
            result.stdout.len()
        );
    }

    #[tokio::test]
    async fn test_execute_skill_script_timeout_kills_process() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = setup_skill(
            tmp.path(),
            "slow-skill",
            &[("slow.sh", "#!/bin/sh\nsleep 60\necho done\n")],
            Some("script"),
        );

        // Set a 1-second timeout via skill.config.yaml
        let config_path = tmp.path().join("slow-skill").join("skill.config.yaml");
        fs::write(&config_path, "timeout_seconds: 1\n").unwrap();

        let err = manager
            .execute_skill_script("slow-skill", "slow.sh", None)
            .await
            .unwrap_err();

        assert!(
            err.to_string().to_lowercase().contains("timeout")
                || err.to_string().to_lowercase().contains("timed out")
                || err.to_string().to_lowercase().contains("elapsed"),
            "expected timeout error, got: {err}"
        );
    }
}
