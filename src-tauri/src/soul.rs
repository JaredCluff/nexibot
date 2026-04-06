///! Soul Module - Persistent Identity and Values for NexiBot
///!
///! Implements the SOUL.md system inspired by OpenClaw:
///! - Loads SOUL.md on startup with a time-based cache (re-reads at most once per 60s)
///! - Provides soul content to Claude as system context
///! - Allows soul modification with version tracking
///! - Supports template management
///! - Enforces a 64 KB size limit on SOUL.md content
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};
use std::time::{Duration, Instant};
use tracing::{info, warn};

/// Maximum allowed size of SOUL.md content in bytes (64 KB).
const MAX_SOUL_SIZE_BYTES: usize = 65_536;

/// How long to cache the soul content before re-reading from disk.
const CACHE_TTL: Duration = Duration::from_secs(60);

/// Module-level cache: (path, content, time_of_last_read).
/// Using std::sync::RwLock so this can be accessed from synchronous `load()`.
static SOUL_CACHE: OnceLock<RwLock<Option<(PathBuf, String, Instant)>>> = OnceLock::new();

fn soul_cache() -> &'static RwLock<Option<(PathBuf, String, Instant)>> {
    SOUL_CACHE.get_or_init(|| RwLock::new(None))
}

/// Soul configuration and content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Soul {
    /// Path to the active SOUL.md file
    pub path: PathBuf,
    /// Content of SOUL.md
    pub content: String,
    /// Last modified timestamp
    pub last_modified: String,
    /// Version identifier
    pub version: String,
}

impl Soul {
    /// Get the soul directory path
    pub fn get_soul_dir() -> Result<PathBuf> {
        let home = dirs::home_dir().context("Failed to get home directory")?;
        Ok(home.join(".config/nexibot/soul"))
    }

    /// Get the templates directory path
    pub fn get_templates_dir() -> Result<PathBuf> {
        Ok(Self::get_soul_dir()?.join("templates"))
    }

    /// Load the active SOUL.md, using the in-process cache when possible.
    ///
    /// The file is read from disk at most once every 60 seconds.
    /// Content larger than 64 KB is truncated with a warning.
    pub fn load() -> Result<Self> {
        let soul_path = Self::get_soul_dir()?.join("SOUL.md");

        if !soul_path.exists() {
            warn!("[SOUL] SOUL.md not found at {:?}, using default", soul_path);
            return Self::create_default();
        }

        let content = Self::load_content_cached(&soul_path)?;
        let version = Self::extract_version(&content);
        let last_modified = Self::extract_last_modified(&content);

        info!("[SOUL] Loaded SOUL.md (version: {})", version);

        Ok(Soul {
            path: soul_path,
            content,
            last_modified,
            version,
        })
    }

    /// Return cached content or read from disk, enforcing the size limit.
    fn load_content_cached(soul_path: &PathBuf) -> Result<String> {
        // Fast path: return cached content if it is still fresh
        {
            let guard = soul_cache().read().unwrap_or_else(|e| e.into_inner());
            if let Some((ref cached_path, ref cached_content, loaded_at)) = *guard {
                if cached_path == soul_path && loaded_at.elapsed() < CACHE_TTL {
                    return Ok(cached_content.clone());
                }
            }
        }

        // Cache miss or stale — read from disk
        let raw = fs::read_to_string(soul_path)
            .with_context(|| format!("Failed to read SOUL.md from {:?}", soul_path))?;

        let content = if raw.len() > MAX_SOUL_SIZE_BYTES {
            warn!(
                "[SOUL] SOUL.md is {} bytes, exceeds limit of {} bytes — truncating",
                raw.len(),
                MAX_SOUL_SIZE_BYTES
            );
            // Truncate at the last valid UTF-8 char boundary at or before the limit
            let mut end = MAX_SOUL_SIZE_BYTES;
            while end > 0 && !raw.is_char_boundary(end) {
                end -= 1;
            }
            raw[..end].to_string()
        } else {
            raw
        };

        // Populate / refresh the cache
        {
            let mut guard = soul_cache().write().unwrap_or_else(|e| e.into_inner());
            *guard = Some((soul_path.clone(), content.clone(), Instant::now()));
        }

        Ok(content)
    }

    /// Invalidate the in-process soul cache.
    ///
    /// Call this after writing a new version of SOUL.md so the next `load()` picks
    /// up the changes immediately instead of waiting for the TTL to expire.
    fn invalidate_cache() {
        let mut guard = soul_cache().write().unwrap_or_else(|e| e.into_inner());
        *guard = None;
    }

    /// Create a default soul file
    fn create_default() -> Result<Self> {
        let soul_dir = Self::get_soul_dir()?;
        fs::create_dir_all(&soul_dir)?;

        let soul_path = soul_dir.join("SOUL.md");
        let default_content = include_str!("../soul/default-soul.md");

        fs::write(&soul_path, default_content)?;

        info!("[SOUL] Created default SOUL.md at {:?}", soul_path);

        Ok(Soul {
            path: soul_path,
            content: default_content.to_string(),
            last_modified: chrono::Utc::now().format("%Y-%m-%d").to_string(),
            version: "1.0.0".to_string(),
        })
    }

    /// Save soul content (for when agent modifies itself)
    pub fn save(&self) -> Result<()> {
        fs::write(&self.path, &self.content)
            .with_context(|| format!("Failed to save SOUL.md to {:?}", self.path))?;

        // Invalidate cache so the next load() picks up the new content immediately
        Self::invalidate_cache();

        info!("[SOUL] Saved updated SOUL.md");
        Ok(())
    }

    /// Update soul content and save
    pub fn update(&mut self, new_content: String) -> Result<()> {
        let previous_content = self.content.clone();
        let previous_last_modified = self.last_modified.clone();
        let previous_version = self.version.clone();

        self.content = new_content;
        self.last_modified = chrono::Utc::now().format("%Y-%m-%d").to_string();

        // Try to extract version from new content
        self.version = Self::extract_version(&self.content);

        if let Err(e) = self.save() {
            self.content = previous_content;
            self.last_modified = previous_last_modified;
            self.version = previous_version;
            return Err(e);
        }
        Ok(())
    }

    /// Get soul content formatted for inclusion in system prompt
    pub fn get_system_prompt_context(&self) -> String {
        format!(
            "# My Soul (Persistent Identity)\n\n{}\n\n---\n\
            I read this SOUL.md at the start of every interaction to maintain consistency.\n\
            If I need to evolve my values or identity, I can request to update this file.\n",
            self.content
        )
    }

    /// List available soul templates
    pub fn list_templates() -> Result<Vec<SoulTemplate>> {
        let templates_dir = Self::get_templates_dir()?;

        if !templates_dir.exists() {
            return Ok(Vec::new());
        }

        let mut templates = Vec::new();

        for entry in fs::read_dir(templates_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().and_then(|s| s.to_str()) == Some("md") {
                let name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown")
                    .to_string();

                let content = fs::read_to_string(&path)?;
                let description = Self::extract_description(&content);

                templates.push(SoulTemplate {
                    name,
                    description,
                    path,
                });
            }
        }

        Ok(templates)
    }

    /// Load a soul from a specific path (for per-agent SOULs).
    pub fn load_from_path(path: &std::path::Path) -> Result<Self> {
        if !path.exists() {
            anyhow::bail!("Soul file not found at {:?}", path);
        }

        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read soul from {:?}", path))?;

        let version = Self::extract_version(&content);
        let last_modified = Self::extract_last_modified(&content);

        info!("[SOUL] Loaded soul from {:?} (version: {})", path, version);

        Ok(Soul {
            path: path.to_path_buf(),
            content,
            last_modified,
            version,
        })
    }

    /// Load a template and set it as the active soul
    pub fn load_template(template_name: &str) -> Result<Self> {
        // Reject names with path separators or parent directory references.
        if template_name.is_empty()
            || !template_name
                .chars()
                .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        {
            anyhow::bail!(
                "Invalid template name '{}': only alphanumeric characters, hyphens, \
                 and underscores are allowed",
                template_name
            );
        }
        let templates_dir = Self::get_templates_dir()?;
        let template_path = templates_dir.join(format!("{}.md", template_name));

        if !template_path.exists() {
            anyhow::bail!("Template '{}' not found", template_name);
        }

        let content = fs::read_to_string(&template_path)?;

        // Copy template to active SOUL.md
        let soul_path = Self::get_soul_dir()?.join("SOUL.md");
        fs::write(&soul_path, &content)?;
        Self::invalidate_cache();

        info!("[SOUL] Loaded template '{}' as active soul", template_name);

        Self::load()
    }

    /// Extract version from soul content
    fn extract_version(content: &str) -> String {
        for line in content.lines() {
            if line.contains("**Version:**") || line.contains("Version:") {
                // Use split_once to correctly handle values that contain colons
                // (e.g., ISO timestamps like "2025-01-15T10:30:45+00:00")
                if let Some((_, val)) = line.split_once(':') {
                    let trimmed = val.trim();
                    if !trimmed.is_empty() {
                        return trimmed.to_string();
                    }
                }
            }
        }
        "1.0.0".to_string()
    }

    /// Extract last modified date from soul content
    fn extract_last_modified(content: &str) -> String {
        for line in content.lines() {
            if line.contains("**Last Modified:**") || line.contains("Last Modified:") {
                // Use split_once to correctly handle timestamps that contain colons
                if let Some((_, val)) = line.split_once(':') {
                    let trimmed = val.trim();
                    if !trimmed.is_empty() {
                        return trimmed.to_string();
                    }
                }
            }
        }
        chrono::Utc::now().format("%Y-%m-%d").to_string()
    }

    /// Extract description from soul content (first paragraph or subtitle)
    fn extract_description(content: &str) -> String {
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('>') {
                return trimmed.trim_start_matches('>').trim().to_string();
            }
        }
        "No description available".to_string()
    }
}

/// Soul template information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoulTemplate {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
}
