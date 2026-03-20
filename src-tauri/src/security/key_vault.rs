//! Smart Key Vault — encrypt real API keys, expose proxy keys to the LLM.
//!
//! Real keys are stored AES-256-GCM encrypted in a local SQLite database.
//! The model always sees format-mimicking proxy keys. When proxy keys appear
//! in outgoing tool calls or HTTP requests, they are silently restored to the
//! real key before execution.
//!
//! # Bidirectional flow
//! ```text
//! User pastes sk-ant-api03-XYZ into chat
//!   → scan_and_replace: real=sk-ant-api03-XYZ ↔ proxy=sk-ant-PROXY-7f3a9b2c...
//!   → model sees: sk-ant-PROXY-7f3a9b2c...
//!
//! Model tool call with sk-ant-PROXY-7f3a9b2c...
//!   → restore_in_json: proxy → sk-ant-api03-XYZ
//!   → tool executes with real key
//! ```

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use rand::Rng;
use regex::Regex;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;
use tracing::{debug, info, warn};
use zeroize::Zeroizing;

use crate::security::log_redactor::{extract_secrets, KeyFormat};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Vault entry info for UI display. Real key is never included.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultEntry {
    #[allow(dead_code)]
    pub proxy_key: String,
    #[allow(dead_code)]
    pub format: String,
    pub label: Option<String>,
    pub created_at: String,
    pub last_used: Option<String>,
    pub use_count: i64,
}

/// Record of a single interception (real key → proxy key substitution).
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Interception {
    pub proxy_key: String,
    pub format: KeyFormat,
}

// ---------------------------------------------------------------------------
// Proxy key patterns (for detection in text)
// ---------------------------------------------------------------------------

static PROXY_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        Regex::new(r"sk-ant-PROXY-[0-9a-f]{32}").expect("invariant: literal regex is valid"),
        Regex::new(r"sk-PROXY[0-9a-f]{32}").expect("invariant: literal regex is valid"),
        Regex::new(r"csk-PROXY[0-9a-f]{32}").expect("invariant: literal regex is valid"),
        Regex::new(r"ghp_PROXY[A-Za-z0-9]{32}").expect("invariant: literal regex is valid"),
        Regex::new(r"AKIAPROXY[A-Z0-9]{12}").expect("invariant: literal regex is valid"),
        Regex::new(r"AIzaPROXY[A-Za-z0-9]{32}").expect("invariant: literal regex is valid"),
        Regex::new(r"xoxb-PROXY-[0-9]{12}").expect("invariant: literal regex is valid"),
        Regex::new(r"MToken-PROXY-[0-9a-f]{32}").expect("invariant: literal regex is valid"),
        Regex::new(r"Bearer PROXY-[0-9a-f]{32}").expect("invariant: literal regex is valid"),
        Regex::new(r"sk_live_PROXY[A-Za-z0-9]{32}").expect("invariant: literal regex is valid"),
        Regex::new(r"pkey_[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")
            .expect("invariant: literal regex is valid"),
    ]
});

// ---------------------------------------------------------------------------
// KeyVault
// ---------------------------------------------------------------------------

/// Smart Key Vault — stores real keys encrypted, returns proxy keys to the LLM.
pub struct KeyVault {
    db: Arc<Mutex<Connection>>,
    passphrase: Zeroizing<String>,
    /// In-memory cache: proxy_key → real_key (decrypted, zeroed on drop/eviction)
    cache: Arc<Mutex<HashMap<String, Zeroizing<String>>>>,
    pub enabled: bool,
}

impl KeyVault {
    /// Open or create the vault database.
    ///
    /// `passphrase` is used for AES-256-GCM encryption of stored keys via
    /// `SessionEncryptor`. `enabled` controls whether interception is active.
    pub fn new(db_path: std::path::PathBuf, passphrase: &str, enabled: bool) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&db_path)?;
        Self::init_schema(&conn)?;

        let vault = Self {
            db: Arc::new(Mutex::new(conn)),
            passphrase: Zeroizing::new(passphrase.to_string()),
            cache: Arc::new(Mutex::new(HashMap::new())),
            enabled,
        };

        vault.warm_cache()?;

        info!(
            "[KEY_VAULT] Initialized (enabled: {}, db: {:?})",
            enabled, db_path
        );
        Ok(vault)
    }

    fn init_schema(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS key_vault (
                proxy_key     TEXT PRIMARY KEY,
                real_key_enc  TEXT NOT NULL,
                format        TEXT NOT NULL,
                label         TEXT,
                created_at    TEXT NOT NULL,
                last_used     TEXT,
                use_count     INTEGER DEFAULT 0
            );
            PRAGMA journal_mode=WAL;",
        )?;
        Ok(())
    }

    /// Load all entries from DB and decrypt into the in-memory caches.
    fn warm_cache(&self) -> Result<()> {
        let conn = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("DB lock poisoned"))?;
        let mut stmt = conn.prepare("SELECT proxy_key, real_key_enc FROM key_vault")?;
        let rows: Vec<(String, String)> = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .filter_map(|r| r.ok())
            .collect();
        drop(stmt);
        drop(conn);

        let encryptor =
            crate::security::session_encryption::SessionEncryptor::new(&self.passphrase, true);
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| anyhow::anyhow!("Cache lock poisoned"))?;

        let mut migrated = Vec::new();
        for (proxy, encrypted) in rows {
            match encryptor.decrypt_line(&encrypted) {
                Ok(real) => {
                    cache.insert(proxy, Zeroizing::new(real));
                }
                Err(_) => {
                    // Try legacy Argon2 parameters (pre-OWASP change)
                    match encryptor.decrypt_line_legacy(&encrypted) {
                        Ok(real) => {
                            info!(
                                "[KEY_VAULT] Migrating vault entry {} from legacy KDF params",
                                &proxy[..proxy.len().min(24)]
                            );
                            cache.insert(proxy.clone(), Zeroizing::new(real.clone()));
                            migrated.push((proxy, real));
                        }
                        Err(e) => {
                            warn!(
                                "[KEY_VAULT] Failed to decrypt vault entry {}: {}",
                                &proxy[..proxy.len().min(20)],
                                e
                            );
                        }
                    }
                }
            }
        }
        drop(cache);

        // Re-encrypt migrated entries with current KDF parameters
        for (proxy, real) in migrated {
            match encryptor.encrypt_line(&real) {
                Ok(new_encrypted) => {
                    let conn = self
                        .db
                        .lock()
                        .map_err(|_| anyhow::anyhow!("DB lock poisoned"))?;
                    if let Err(e) = conn.execute(
                        "UPDATE key_vault SET real_key_enc = ?1 WHERE proxy_key = ?2",
                        params![new_encrypted, proxy],
                    ) {
                        warn!(
                            "[KEY_VAULT] Failed to re-encrypt migrated entry {}: {}",
                            &proxy[..proxy.len().min(24)],
                            e
                        );
                    } else {
                        info!(
                            "[KEY_VAULT] Successfully migrated entry {} to current KDF params",
                            &proxy[..proxy.len().min(24)]
                        );
                    }
                }
                Err(e) => {
                    warn!(
                        "[KEY_VAULT] Failed to re-encrypt migrated entry: {}",
                        e
                    );
                }
            }
        }

        Ok(())
    }

    /// Store a real key in the vault and return its proxy key.
    ///
    /// If the key is already stored, returns the existing proxy key without
    /// re-encrypting.
    pub fn store(&self, real_key: &str, label: Option<&str>) -> Result<String> {
        // Scan cache for an existing entry with this real key (avoids duplicates).
        // The vault typically holds few keys so a linear scan is acceptable.
        {
            let cache = self
                .cache
                .lock()
                .map_err(|_| anyhow::anyhow!("Lock poisoned"))?;
            if let Some((proxy, _)) = cache.iter().find(|(_, v)| v.as_str() == real_key) {
                return Ok(proxy.clone());
            }
        }

        let format = detect_format(real_key);
        let proxy_key = generate_proxy_key(&format);
        let encryptor =
            crate::security::session_encryption::SessionEncryptor::new(&self.passphrase, true);
        let encrypted = encryptor.encrypt_line(real_key)?;
        let now = chrono::Utc::now().to_rfc3339();

        {
            let conn = self
                .db
                .lock()
                .map_err(|_| anyhow::anyhow!("DB lock poisoned"))?;
            conn.execute(
                "INSERT OR REPLACE INTO key_vault (proxy_key, real_key_enc, format, label, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![proxy_key, encrypted, format.as_str(), label, now],
            )?;
        }

        // Update in-memory cache
        {
            let mut cache = self
                .cache
                .lock()
                .map_err(|_| anyhow::anyhow!("Lock poisoned"))?;
            cache.insert(proxy_key.clone(), Zeroizing::new(real_key.to_string()));
        }

        info!(
            "[KEY_VAULT] Stored {} key as proxy {}",
            format.as_str(),
            &proxy_key[..proxy_key.len().min(24)]
        );
        Ok(proxy_key)
    }

    /// Resolve a proxy key back to the real key.
    ///
    /// Returns `None` if the proxy key is not in the vault (e.g., revoked).
    pub fn resolve(&self, proxy_key: &str) -> Result<Option<String>> {
        // Check in-memory cache first (fast path)
        {
            let cache = self
                .cache
                .lock()
                .map_err(|_| anyhow::anyhow!("Lock poisoned"))?;
            if let Some(real) = cache.get(proxy_key) {
                let real = (**real).clone();
                drop(cache);
                self.bump_use_count(proxy_key);
                return Ok(Some(real));
            }
        }

        // DB fallback (should not be needed after warm_cache, but kept as safety net)
        let encrypted = {
            let conn = self
                .db
                .lock()
                .map_err(|_| anyhow::anyhow!("DB lock poisoned"))?;
            conn.query_row(
                "SELECT real_key_enc FROM key_vault WHERE proxy_key = ?1",
                params![proxy_key],
                |row| row.get::<_, String>(0),
            )
            .ok()
        };

        if let Some(enc) = encrypted {
            let encryptor =
                crate::security::session_encryption::SessionEncryptor::new(&self.passphrase, true);
            let real = match encryptor.decrypt_line(&enc) {
                Ok(r) => r,
                Err(_) => {
                    // Try legacy Argon2 parameters as fallback
                    encryptor.decrypt_line_legacy(&enc)?
                }
            };
            {
                let mut cache = self
                    .cache
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Lock poisoned"))?;
                cache.insert(proxy_key.to_string(), Zeroizing::new(real.clone()));
            }
            self.bump_use_count(proxy_key);
            Ok(Some(real))
        } else {
            Ok(None)
        }
    }

    /// Return all proxy→real mappings from the in-memory cache.
    ///
    /// Used by NexiGate to pre-populate its bidirectional filter layer.
    pub fn all_proxy_to_real(&self) -> std::collections::HashMap<String, String> {
        self.cache
            .lock()
            .map(|c| c.iter().map(|(k, v)| (k.clone(), (**v).clone())).collect())
            .unwrap_or_default()
    }

    /// Fast check: does this string look like a vault-issued proxy key?
    pub fn is_proxy_key(s: &str) -> bool {
        s.contains("PROXY") || s.starts_with("pkey_")
    }

    /// List all vault entries for UI display. Real keys are never included.
    pub fn list(&self) -> Vec<VaultEntry> {
        let conn = match self.db.lock() {
            Ok(c) => c,
            Err(_) => return vec![],
        };
        let mut stmt = match conn.prepare(
            "SELECT proxy_key, format, label, created_at, last_used, use_count \
             FROM key_vault ORDER BY created_at DESC",
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        stmt.query_map([], |row| {
            Ok(VaultEntry {
                proxy_key: row.get(0)?,
                format: row.get(1)?,
                label: row.get(2)?,
                created_at: row.get(3)?,
                last_used: row.get(4)?,
                use_count: row.get(5)?,
            })
        })
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    /// Revoke a proxy key. Future `resolve` calls for this key return `None`.
    pub fn revoke(&self, proxy_key: &str) -> Result<()> {
        {
            let conn = self
                .db
                .lock()
                .map_err(|_| anyhow::anyhow!("DB lock poisoned"))?;
            conn.execute(
                "DELETE FROM key_vault WHERE proxy_key = ?1",
                params![proxy_key],
            )?;
        }
        if let Ok(mut cache) = self.cache.lock() {
            // Remove and drop: Zeroizing<String> value is zeroed on drop
            cache.remove(proxy_key);
        }
        info!(
            "[KEY_VAULT] Revoked proxy key {}",
            &proxy_key[..proxy_key.len().min(24)]
        );
        Ok(())
    }

    /// Set a user-assigned label on a vault entry.
    pub fn set_label(&self, proxy_key: &str, label: &str) -> Result<()> {
        let conn = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("DB lock poisoned"))?;
        conn.execute(
            "UPDATE key_vault SET label = ?1 WHERE proxy_key = ?2",
            params![label, proxy_key],
        )?;
        Ok(())
    }

    /// Scan text for real API keys; store each in the vault; replace with proxy keys.
    ///
    /// Returns the sanitized text (safe to show the model) and a list of
    /// intercepted key records.
    pub fn scan_and_replace(&self, text: &str) -> (String, Vec<Interception>) {
        if !self.enabled {
            return (text.to_string(), vec![]);
        }

        let secret_matches = extract_secrets(text);
        if secret_matches.is_empty() {
            return (text.to_string(), vec![]);
        }

        // Work on a mutable copy. Replace in reverse order to keep offsets valid.
        let mut result = text.to_string();
        let mut interceptions = Vec::new();

        for m in secret_matches.iter().rev() {
            if Self::is_proxy_key(&m.value) {
                continue;
            }
            match self.store(&m.value, None) {
                Ok(proxy) => {
                    result.replace_range(m.start..m.end, &proxy);
                    interceptions.push(Interception {
                        proxy_key: proxy,
                        format: m.format.clone(),
                    });
                }
                Err(e) => {
                    warn!("[KEY_VAULT] Failed to store intercepted key: {}", e);
                }
            }
        }

        if !interceptions.is_empty() {
            debug!(
                "[KEY_VAULT] scan_and_replace: {} keys intercepted",
                interceptions.len()
            );
        }
        (result, interceptions)
    }

    /// Scan text for proxy keys and restore them to real keys.
    pub fn restore_in_text(&self, text: &str) -> String {
        if !self.enabled {
            return text.to_string();
        }

        let proxies = find_proxy_keys_in(text);
        if proxies.is_empty() {
            return text.to_string();
        }

        let mut result = text.to_string();
        // Reverse order so offsets stay valid during replacement
        for (start, end, proxy) in proxies.iter().rev() {
            match self.resolve(proxy) {
                Ok(Some(real)) => {
                    result.replace_range(*start..*end, &real);
                    debug!("[KEY_VAULT] Restored proxy key in text");
                }
                Ok(None) => {
                    warn!(
                        "[KEY_VAULT] Proxy key not in vault (revoked?): {}",
                        &proxy[..proxy.len().min(24)]
                    );
                }
                Err(e) => {
                    warn!("[KEY_VAULT] Failed to resolve proxy key: {}", e);
                }
            }
        }
        result
    }

    /// Recursively restore proxy keys in a `serde_json::Value` (for tool inputs).
    pub fn restore_in_json(&self, value: &mut serde_json::Value) {
        if !self.enabled {
            return;
        }
        match value {
            serde_json::Value::String(s) => {
                let restored = self.restore_in_text(s);
                *s = restored;
            }
            serde_json::Value::Object(map) => {
                for v in map.values_mut() {
                    self.restore_in_json(v);
                }
            }
            serde_json::Value::Array(arr) => {
                for v in arr.iter_mut() {
                    self.restore_in_json(v);
                }
            }
            _ => {}
        }
    }

    fn bump_use_count(&self, proxy_key: &str) {
        if let Ok(conn) = self.db.lock() {
            let now = chrono::Utc::now().to_rfc3339();
            let _ = conn.execute(
                "UPDATE key_vault SET use_count = use_count + 1, last_used = ?1 WHERE proxy_key = ?2",
                params![now, proxy_key],
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Format detection
// ---------------------------------------------------------------------------

/// Detect the key format from its prefix / shape.
pub fn detect_format(key: &str) -> KeyFormat {
    if key.starts_with("sk-ant-") {
        KeyFormat::Anthropic
    } else if key.starts_with("sk_live_")
        || key.starts_with("sk_test_")
        || key.starts_with("pk_live_")
        || key.starts_with("pk_test_")
    {
        KeyFormat::Stripe
    } else if key.starts_with("csk-") {
        KeyFormat::Cerebras
    } else if key.starts_with("sk-") {
        KeyFormat::OpenAI
    } else if key.starts_with("ghp_") || key.starts_with("ghs_") {
        KeyFormat::GitHub
    } else if key.starts_with("AKIA") {
        KeyFormat::Aws
    } else if key.starts_with("AIza") {
        KeyFormat::Google
    } else if key.starts_with("xox") {
        KeyFormat::Slack
    } else if (key.starts_with('M') || key.starts_with('N'))
        && key.contains('.')
        && key.split('.').count() == 3
        && key.len() > 50
    {
        KeyFormat::Discord
    } else if key.starts_with("Bearer ") {
        KeyFormat::Bearer
    } else {
        KeyFormat::Unknown
    }
}

// ---------------------------------------------------------------------------
// Proxy key generation
// ---------------------------------------------------------------------------

fn hex32() -> String {
    let mut rng = rand::rngs::OsRng;
    (0..16)
        .map(|_| format!("{:02x}", rng.gen::<u8>()))
        .collect()
}

fn alphanum32() -> String {
    let charset = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::rngs::OsRng;
    (0..32)
        .map(|_| charset[rng.gen_range(0..charset.len())] as char)
        .collect()
}

fn upper_alphanum12() -> String {
    let charset = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rng = rand::rngs::OsRng;
    (0..12)
        .map(|_| charset[rng.gen_range(0..charset.len())] as char)
        .collect()
}

fn digits12() -> String {
    let mut rng = rand::rngs::OsRng;
    (0..12)
        .map(|_| rng.gen_range(0u8..10).to_string())
        .collect::<Vec<_>>()
        .join("")
}

/// Generate a proxy key that visually mimics the format of the original.
pub fn generate_proxy_key(format: &KeyFormat) -> String {
    match format {
        KeyFormat::Anthropic => format!("sk-ant-PROXY-{}", hex32()),
        KeyFormat::OpenAI => format!("sk-PROXY{}", hex32()),
        KeyFormat::Cerebras => format!("csk-PROXY{}", hex32()),
        KeyFormat::GitHub => format!("ghp_PROXY{}", alphanum32()),
        KeyFormat::Aws => format!("AKIAPROXY{}", upper_alphanum12()),
        KeyFormat::Google => format!("AIzaPROXY{}", alphanum32()),
        KeyFormat::Slack => format!("xoxb-PROXY-{}", digits12()),
        KeyFormat::Discord => format!("MToken-PROXY-{}", hex32()),
        KeyFormat::Bearer => format!("Bearer PROXY-{}", hex32()),
        KeyFormat::Stripe => format!("sk_live_PROXY{}", alphanum32()),
        KeyFormat::Unknown => format!("pkey_{}", uuid::Uuid::new_v4()),
    }
}

// ---------------------------------------------------------------------------
// Proxy key detection in text
// ---------------------------------------------------------------------------

/// Find all proxy keys in text, returning (start, end, proxy_key) sorted
/// in descending order by start offset (ready for right-to-left replacement).
fn find_proxy_keys_in(text: &str) -> Vec<(usize, usize, String)> {
    let mut found: Vec<(usize, usize, String)> = Vec::new();
    let mut covered: Vec<(usize, usize)> = Vec::new();

    for pattern in PROXY_PATTERNS.iter() {
        for m in pattern.find_iter(text) {
            let start = m.start();
            let end = m.end();
            let overlaps = covered.iter().any(|&(cs, ce)| start < ce && end > cs);
            if overlaps {
                continue;
            }
            covered.push((start, end));
            found.push((start, end, m.as_str().to_string()));
        }
    }

    // Sort descending by start (reverse order for safe in-place replacement)
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found
}

// ---------------------------------------------------------------------------
// KeyFormat helper (shared with log_redactor)
// ---------------------------------------------------------------------------

impl KeyFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            KeyFormat::Anthropic => "anthropic",
            KeyFormat::OpenAI => "openai",
            KeyFormat::Cerebras => "cerebras",
            KeyFormat::GitHub => "github",
            KeyFormat::Aws => "aws",
            KeyFormat::Google => "google",
            KeyFormat::Slack => "slack",
            KeyFormat::Discord => "discord",
            KeyFormat::Bearer => "bearer",
            KeyFormat::Stripe => "stripe",
            KeyFormat::Unknown => "unknown",
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_vault() -> KeyVault {
        let dir = tempdir().unwrap();
        let db = dir.path().join("test_vault.sqlite");
        // Keep tempdir alive by leaking (acceptable in tests)
        std::mem::forget(dir);
        KeyVault::new(db, "test-passphrase-12345", true).unwrap()
    }

    #[test]
    fn test_store_and_resolve() {
        let vault = make_vault();
        let real = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz";
        let proxy = vault.store(real, None).unwrap();
        assert!(proxy.starts_with("sk-ant-PROXY-"), "proxy: {}", proxy);
        let resolved = vault.resolve(&proxy).unwrap();
        assert_eq!(resolved, Some(real.to_string()));
    }

    #[test]
    fn test_duplicate_store_returns_same_proxy() {
        let vault = make_vault();
        let real = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz";
        let p1 = vault.store(real, None).unwrap();
        let p2 = vault.store(real, None).unwrap();
        assert_eq!(p1, p2);
    }

    #[test]
    fn test_scan_and_replace_anthropic() {
        let vault = make_vault();
        let text = "My API key is sk-ant-api03-abcdefghijklmnopqrstu and nothing else.";
        let (sanitized, interceptions) = vault.scan_and_replace(text);
        assert_eq!(interceptions.len(), 1);
        assert!(sanitized.contains("sk-ant-PROXY-"));
        assert!(!sanitized.contains("sk-ant-api03-"));
    }

    #[test]
    fn test_restore_in_text() {
        let vault = make_vault();
        let real = "sk-ant-api03-abcdefghijklmnopqrstu";
        let proxy = vault.store(real, None).unwrap();
        let text = format!("Use key {} for auth", proxy);
        let restored = vault.restore_in_text(&text);
        assert!(restored.contains(real));
        assert!(!restored.contains("PROXY"));
    }

    #[test]
    fn test_restore_in_json() {
        let vault = make_vault();
        let real = "sk-PROXY_real_key_abcdefghijklmnopq";
        // Store a real OpenAI key
        let real_key = "sk-abcdefghijklmnopqrstuvwxyz1234";
        let proxy = vault.store(real_key, None).unwrap();
        let mut val = serde_json::json!({"api_key": proxy, "other": "data"});
        vault.restore_in_json(&mut val);
        assert_eq!(val["api_key"].as_str().unwrap(), real_key);
        assert_eq!(val["other"].as_str().unwrap(), "data");
        let _ = real; // suppress unused warning
    }

    #[test]
    fn test_revoke() {
        let vault = make_vault();
        let real = "sk-ant-api03-abcdefghijklmnopqrstu";
        let proxy = vault.store(real, None).unwrap();
        vault.revoke(&proxy).unwrap();
        let resolved = vault.resolve(&proxy).unwrap();
        assert_eq!(resolved, None);
    }

    #[test]
    fn test_is_proxy_key() {
        assert!(KeyVault::is_proxy_key("sk-ant-PROXY-abc123"));
        assert!(KeyVault::is_proxy_key(
            "pkey_12345678-0000-0000-0000-000000000000"
        ));
        assert!(!KeyVault::is_proxy_key("sk-ant-api03-realkey"));
    }

    #[test]
    fn test_detect_format() {
        assert_eq!(detect_format("sk-ant-api03-xyz"), KeyFormat::Anthropic);
        assert_eq!(detect_format("sk-abcdef12345"), KeyFormat::OpenAI);
        assert_eq!(detect_format("csk-abcdef12345"), KeyFormat::Cerebras);
        assert_eq!(detect_format("ghp_abcdef12345"), KeyFormat::GitHub);
        assert_eq!(detect_format("AKIA1234567890ABCD"), KeyFormat::Aws);
        assert_eq!(detect_format("AIza1234567890abcde"), KeyFormat::Google);
        assert_eq!(detect_format("xoxb-123-abc"), KeyFormat::Slack);
        assert_eq!(detect_format("Bearer eyJabc"), KeyFormat::Bearer);
        assert_eq!(detect_format("sk_live_abcdef12345"), KeyFormat::Stripe);
        assert_eq!(detect_format("completely_unknown"), KeyFormat::Unknown);
    }

    #[test]
    fn test_proxy_key_not_intercepted_twice() {
        let vault = make_vault();
        let real = "sk-ant-api03-abcdefghijklmnopqrstu";
        let proxy = vault.store(real, None).unwrap();
        // Scan text that already has the proxy key — should not double-intercept
        let (sanitized, interceptions) = vault.scan_and_replace(&proxy);
        assert_eq!(interceptions.len(), 0);
        assert_eq!(sanitized, proxy);
    }

    #[test]
    fn test_disabled_vault_is_noop() {
        let dir = tempdir().unwrap();
        let db = dir.path().join("test_vault.sqlite");
        let vault = KeyVault::new(db, "passphrase", false).unwrap();
        let text = "key sk-ant-api03-abcdefghijklmnopqrstu end";
        let (sanitized, interceptions) = vault.scan_and_replace(text);
        assert_eq!(sanitized, text);
        assert_eq!(interceptions.len(), 0);
        std::mem::forget(dir);
    }

    #[test]
    fn test_generate_proxy_key_formats() {
        let ant = generate_proxy_key(&KeyFormat::Anthropic);
        assert!(ant.starts_with("sk-ant-PROXY-"), "{}", ant);
        let oai = generate_proxy_key(&KeyFormat::OpenAI);
        assert!(oai.starts_with("sk-PROXY"), "{}", oai);
        let gh = generate_proxy_key(&KeyFormat::GitHub);
        assert!(gh.starts_with("ghp_PROXY"), "{}", gh);
        let aws = generate_proxy_key(&KeyFormat::Aws);
        assert!(aws.starts_with("AKIAPROXY"), "{}", aws);
        let pkey = generate_proxy_key(&KeyFormat::Unknown);
        assert!(pkey.starts_with("pkey_"), "{}", pkey);
    }
}
