//! ClawHub Marketplace Client
//!
//! Provides search, detail, and install functionality for OpenClaw-compatible
//! skills from the ClawHub marketplace. Every install runs through the security
//! analysis engine before writing files to disk.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

use crate::skill_security::{self, RiskLevel, SkillSecurityReport};
use crate::skills::{SkillInstallInfo, SkillsManager};

const CLAWHUB_API_BASE: &str = "https://api.clawhub.com/v1";

/// Summary of a skill from ClawHub search results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClawHubSkillSummary {
    pub slug: String,
    pub name: String,
    pub description: String,
    pub author: String,
    pub version: String,
    pub downloads: u64,
    pub rating: f32,
    pub tags: Vec<String>,
}

/// Detailed skill info from ClawHub.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClawHubSkillDetail {
    pub slug: String,
    pub name: String,
    pub description: String,
    pub author: String,
    pub version: String,
    pub downloads: u64,
    pub rating: f32,
    pub tags: Vec<String>,
    /// SKILL.md content
    pub skill_md: String,
    /// Scripts as filename -> content
    pub scripts: HashMap<String, String>,
    /// References as filename -> content
    pub references: HashMap<String, String>,
    /// OpenClaw metadata JSON
    pub metadata: Option<serde_json::Value>,
}

/// Result of an install operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallResult {
    pub success: bool,
    pub skill_id: String,
    pub security_report: SkillSecurityReport,
    pub message: String,
}

// ---------------------------------------------------------------------------
// Skill integrity verification (P8A)
// ---------------------------------------------------------------------------

/// A single integrity record for an installed skill.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillIntegrityEntry {
    /// ClawHub slug or local skill ID.
    pub slug: String,
    /// Installed version string.
    pub version: String,
    /// SHA-256 hex digest of the skill directory contents at install time.
    pub sha256: String,
    /// ISO 8601 timestamp of when the skill was installed.
    pub installed_at: String,
}

/// All integrity entries, keyed by skill ID.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillIntegrityManifest {
    pub entries: HashMap<String, SkillIntegrityEntry>,
}

impl SkillIntegrityManifest {
    /// Path to the integrity manifest file.
    fn manifest_path() -> Result<PathBuf> {
        let skills_dir = SkillsManager::get_skills_dir()?;
        Ok(skills_dir.join(".integrity.json"))
    }

    /// Load the integrity manifest from disk, or return an empty one.
    pub fn load() -> Result<Self> {
        let path = Self::manifest_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = fs::read_to_string(&path).context("Failed to read integrity manifest")?;
        let manifest: Self =
            serde_json::from_str(&content).context("Failed to parse integrity manifest")?;
        Ok(manifest)
    }

    /// Save the integrity manifest to disk.
    pub fn save(&self) -> Result<()> {
        let path = Self::manifest_path()?;
        let content =
            serde_json::to_string_pretty(self).context("Failed to serialize integrity manifest")?;
        fs::write(&path, content).context("Failed to write integrity manifest")?;
        Ok(())
    }

    /// Record the integrity hash for a newly installed skill.
    pub fn record(&mut self, skill_id: &str, entry: SkillIntegrityEntry) {
        self.entries.insert(skill_id.to_string(), entry);
    }

    /// Verify a skill's current on-disk hash against the recorded hash.
    /// Returns Ok(true) if it matches, Ok(false) if it drifted, or an error.
    pub fn verify(&self, skill_id: &str, skill_dir: &Path) -> Result<bool> {
        let entry = match self.entries.get(skill_id) {
            Some(e) => e,
            None => {
                warn!(
                    "[INTEGRITY] No integrity record for skill '{}', skipping verification",
                    skill_id
                );
                return Ok(true); // No record means nothing to verify
            }
        };

        let current_hash = compute_directory_hash(skill_dir)?;
        if current_hash != entry.sha256 {
            warn!(
                "[INTEGRITY] Skill '{}' hash mismatch! Expected {} but got {}. \
                 The skill may have been tampered with since installation.",
                skill_id, entry.sha256, current_hash
            );
            return Ok(false);
        }

        Ok(true)
    }
}

/// Compute a SHA-256 hash of all files in a directory (sorted by relative path).
///
/// This produces a deterministic hash by sorting file paths and hashing
/// each file's relative path + content in sequence.
pub fn compute_directory_hash(dir: &Path) -> Result<String> {
    let mut hasher = Sha256::new();
    let mut file_paths: Vec<PathBuf> = Vec::new();

    collect_files_recursive(dir, &mut file_paths)?;
    file_paths.sort();

    for file_path in &file_paths {
        // Hash the relative path
        if let Ok(relative) = file_path.strip_prefix(dir) {
            hasher.update(relative.to_string_lossy().as_bytes());
        }

        // Hash the file content
        let mut file = fs::File::open(file_path)
            .with_context(|| format!("Failed to open {:?} for hashing", file_path))?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)
            .with_context(|| format!("Failed to read {:?} for hashing", file_path))?;
        hasher.update(&buf);
    }

    let hash = hasher.finalize();
    Ok(format!("{:x}", hash))
}

/// Recursively collect all file paths under a directory.
fn collect_files_recursive(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        // Skip the integrity manifest itself
        if path.file_name().and_then(|n| n.to_str()) == Some(".integrity.json") {
            continue;
        }
        if path.is_dir() {
            collect_files_recursive(&path, out)?;
        } else if path.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// ClawHub client
// ---------------------------------------------------------------------------

/// ClawHub API client.
pub struct ClawHubClient {
    http: reqwest::Client,
}

impl ClawHubClient {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|e| {
                    warn!("[CLAWHUB] Failed to build HTTP client with timeout, using default (requests may hang): {}", e);
                    reqwest::Client::new()
                }),
        }
    }

    /// Search ClawHub marketplace.
    pub async fn search(&self, query: &str, limit: u32) -> Result<Vec<ClawHubSkillSummary>> {
        info!("[CLAWHUB] Searching for: {} (limit: {})", query, limit);

        let resp = self
            .http
            .get(format!("{}/skills/search", CLAWHUB_API_BASE))
            .query(&[("q", query), ("limit", &limit.to_string())])
            .send()
            .await
            .context("ClawHub search request failed")?;

        if !resp.status().is_success() {
            anyhow::bail!("ClawHub API returned status {}", resp.status());
        }

        let results: Vec<ClawHubSkillSummary> = resp
            .json()
            .await
            .context("Failed to parse ClawHub search response")?;

        info!("[CLAWHUB] Found {} results", results.len());
        Ok(results)
    }

    /// Get detailed skill info from ClawHub.
    pub async fn get_skill(&self, slug: &str) -> Result<ClawHubSkillDetail> {
        // Validate slug before inserting into the URL path.
        // A slug containing '/' or '%2F' could traverse to unintended API endpoints.
        if slug.is_empty()
            || !slug
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            anyhow::bail!(
                "Invalid skill slug '{}': only alphanumeric characters, hyphens, and underscores are allowed",
                slug
            );
        }
        info!("[CLAWHUB] Getting skill detail: {}", slug);

        let resp = self
            .http
            .get(format!("{}/skills/{}", CLAWHUB_API_BASE, slug))
            .send()
            .await
            .context("ClawHub get skill request failed")?;

        if !resp.status().is_success() {
            anyhow::bail!(
                "ClawHub API returned status {} for skill '{}'",
                resp.status(),
                slug
            );
        }

        let detail: ClawHubSkillDetail = resp
            .json()
            .await
            .context("Failed to parse ClawHub skill detail")?;

        Ok(detail)
    }

    /// Install a skill from ClawHub with security analysis.
    ///
    /// Flow:
    /// 1. Fetch skill from ClawHub
    /// 2. Run security analysis
    /// 3. Block Dangerous skills unless force_install
    /// 4. Write files to skills directory
    /// 5. Compute and record integrity hash
    /// 6. Return install result with security report
    pub async fn install_skill(
        &self,
        slug: &str,
        skills_manager: &mut SkillsManager,
        force_install: bool,
    ) -> Result<InstallResult> {
        info!(
            "[CLAWHUB] Installing skill: {} (force: {})",
            slug, force_install
        );

        // 1. Fetch skill
        let detail = self.get_skill(slug).await?;

        // 2. Run security analysis
        let scripts_content: String = detail
            .scripts
            .values()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");

        let full_content = format!("{}\n\n{}", detail.skill_md, scripts_content);

        // Parse metadata for requirement analysis
        let temp_metadata = crate::skills::SkillMetadata {
            name: Some(detail.name.clone()),
            description: Some(detail.description.clone()),
            user_invocable: true,
            disable_model_invocation: false,
            requirements: Vec::new(),
            metadata: detail.metadata.clone(),
            command_dispatch: None,
            command_tool: None,
            command_arg_mode: None,
            version: Some(detail.version.clone()),
            author: Some(detail.author.clone()),
            source: Some("clawhub".to_string()),
        };

        let report = skill_security::analyze_skill_content(
            &detail.name,
            &full_content,
            None, // Scripts already included in full_content
            Some(&temp_metadata),
        );

        // 3. Check risk level
        if report.risk_level == RiskLevel::Dangerous && !force_install {
            warn!("[CLAWHUB] Skill '{}' blocked: {}", slug, report.summary);
            return Ok(InstallResult {
                success: false,
                skill_id: slug.to_string(),
                security_report: report,
                message: format!(
                    "Skill '{}' has been flagged as DANGEROUS and cannot be installed without force_install=true. Review the security report carefully before proceeding.",
                    slug
                ),
            });
        }

        if report.risk_level == RiskLevel::Warning {
            warn!(
                "[CLAWHUB] Skill '{}' has warnings: {}",
                slug, report.summary
            );
        }

        // 4. Write files
        let skill_id = slug.replace('/', "-");
        let skills_dir = SkillsManager::get_skills_dir()?;
        let skill_dir = skills_dir.join(&skill_id);

        if skill_dir.exists() {
            anyhow::bail!(
                "Skill '{}' already exists. Delete it first or use a different ID.",
                skill_id
            );
        }

        // Guard against a symlink placed at skill_dir between the exists() check and
        // create_dir_all(). Without this, an attacker with local filesystem access could
        // pre-create a symlink at skill_dir that points outside the skills directory;
        // create_dir_all() would silently succeed (creating directories through the symlink),
        // and all subsequent writes would land at the symlink target instead.
        crate::security::path_validation::reject_symlink(&skill_dir)
            .map_err(|e| anyhow::anyhow!("Skill directory path is a symlink: {}", e))?;

        fs::create_dir_all(&skill_dir)?;

        // Re-verify after creation: if create_dir_all succeeded via a symlink that was
        // placed after the check above, catch it here.
        crate::security::path_validation::reject_symlink(&skill_dir)
            .map_err(|e| anyhow::anyhow!("Skill directory became a symlink after creation: {}", e))?;

        // Write SKILL.md
        fs::write(skill_dir.join("SKILL.md"), &detail.skill_md)?;

        // Write scripts (with path traversal / Zip Slip prevention)
        if !detail.scripts.is_empty() {
            let scripts_dir = skill_dir.join("scripts");
            crate::security::path_validation::reject_symlink(&scripts_dir)
                .map_err(|e| anyhow::anyhow!("scripts_dir is a symlink: {}", e))?;
            fs::create_dir_all(&scripts_dir)?;
            crate::security::path_validation::reject_symlink(&scripts_dir)
                .map_err(|e| anyhow::anyhow!("scripts_dir became a symlink after creation: {}", e))?;
            for (filename, content) in &detail.scripts {
                crate::security::path_validation::validate_skill_filename(filename)
                    .map_err(|e| anyhow::anyhow!("Unsafe script filename '{}': {}", filename, e))?;
                let dest = scripts_dir.join(filename);
                crate::security::path_validation::reject_symlink(&dest)
                    .map_err(|e| anyhow::anyhow!("{}", e))?;
                fs::write(&dest, content)?;
                // Make scripts executable on Unix
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = fs::set_permissions(&dest, fs::Permissions::from_mode(0o700));
                }
            }
        }

        // Write references (with path traversal / Zip Slip prevention)
        if !detail.references.is_empty() {
            let refs_dir = skill_dir.join("references");
            crate::security::path_validation::reject_symlink(&refs_dir)
                .map_err(|e| anyhow::anyhow!("refs_dir is a symlink: {}", e))?;
            fs::create_dir_all(&refs_dir)?;
            crate::security::path_validation::reject_symlink(&refs_dir)
                .map_err(|e| anyhow::anyhow!("refs_dir became a symlink after creation: {}", e))?;
            for (filename, content) in &detail.references {
                crate::security::path_validation::validate_skill_filename(filename).map_err(
                    |e| anyhow::anyhow!("Unsafe reference filename '{}': {}", filename, e),
                )?;
                let dest = refs_dir.join(filename);
                crate::security::path_validation::reject_symlink(&dest)
                    .map_err(|e| anyhow::anyhow!("{}", e))?;
                fs::write(&dest, content)?;
            }
        }

        // Write install info
        let install_info = SkillInstallInfo {
            slug: slug.to_string(),
            version: detail.version.clone(),
            installed_at: chrono::Utc::now().to_rfc3339(),
            security_score: report.overall_score,
        };
        fs::write(
            skill_dir.join("install_info.json"),
            serde_json::to_string_pretty(&install_info)?,
        )?;

        // 5. Compute and record integrity hash (blocking I/O — use block_in_place)
        match tokio::task::block_in_place(|| compute_directory_hash(&skill_dir)) {
            Ok(hash) => {
                let integrity_entry = SkillIntegrityEntry {
                    slug: slug.to_string(),
                    version: detail.version.clone(),
                    sha256: hash.clone(),
                    installed_at: chrono::Utc::now().to_rfc3339(),
                };
                match SkillIntegrityManifest::load() {
                    Ok(mut manifest) => {
                        manifest.record(&skill_id, integrity_entry);
                        if let Err(e) = manifest.save() {
                            warn!("[CLAWHUB] Failed to save integrity manifest: {}", e);
                        } else {
                            info!(
                                "[CLAWHUB] Recorded integrity hash for '{}': {}",
                                skill_id, hash
                            );
                        }
                    }
                    Err(e) => {
                        warn!("[CLAWHUB] Failed to load integrity manifest: {}", e);
                    }
                }
            }
            Err(e) => {
                warn!(
                    "[CLAWHUB] Failed to compute integrity hash for '{}': {}",
                    skill_id, e
                );
            }
        }

        // 6. Reload skills (blocking I/O — use block_in_place)
        if let Err(e) = tokio::task::block_in_place(|| skills_manager.load_all_skills()) {
            warn!("[CLAWHUB] Failed to reload skills after install: {}", e);
        }

        let message = match report.risk_level {
            RiskLevel::Warning => format!(
                "Skill '{}' installed with warnings. Review the security report.",
                skill_id
            ),
            _ => format!(
                "Skill '{}' installed successfully (score: {:.2})",
                skill_id, report.overall_score
            ),
        };

        info!("[CLAWHUB] {}", message);

        Ok(InstallResult {
            success: true,
            skill_id,
            security_report: report,
            message,
        })
    }
}
