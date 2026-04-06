//! Named session management for inter-agent messaging.
//!
//! Supports local named sessions with isolated conversation histories,
//! an internal message bus for cross-session communication, and JSONL-based
//! persistent transcripts in `~/.config/nexibot/sessions/`.
//!
//! ## Cross-context messaging policy
//!
//! Messages sent between sessions are validated for:
//! - Content length bounds (max 10,000 chars)
//! - Sender session existence
//! - Target session existence
//! - Cross-channel boundary markers when sender and target use different
//!   channel types (e.g. telegram -> gui)

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::path::PathBuf;
use tokio::sync::broadcast;
use tracing::{info, warn};

use crate::security::external_content::wrap_external_content;
use crate::security::session_encryption::SessionEncryptor;

/// Maximum number of messages held in any single session's inbox.
/// Oldest messages are drained when this limit is exceeded.
const MAX_INBOX_SIZE: usize = 1000;

/// Maximum number of named sessions. Creation is rejected when the limit is reached.
const MAX_SESSIONS: usize = 100;

/// Maximum length (in chars) of a single inter-session message.
/// Prevents memory exhaustion via unbounded message payloads.
const MAX_MESSAGE_LENGTH: usize = 10_000;

/// A message routed between sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterSessionMessage {
    pub from_session: String,
    pub to_session: String,
    pub content: String,
    pub timestamp: DateTime<Utc>,
}

/// A named session with metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamedSession {
    pub id: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub message_count: usize,
    /// Channel type that owns this session (e.g. "gui", "telegram", "whatsapp").
    /// Used to detect cross-channel messages and apply boundary markers.
    #[serde(default = "default_channel_type")]
    pub channel_type: String,
}

fn default_channel_type() -> String {
    "gui".to_string()
}

/// A single JSONL transcript entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TranscriptEntry {
    #[serde(rename = "type")]
    entry_type: String,
    from: String,
    content: String,
    timestamp: DateTime<Utc>,
    /// Channel type for session_created entries (persisted for reload)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    channel_type: Option<String>,
}

/// Result of a transcript truncation operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TruncationResult {
    /// Number of entries before truncation.
    pub entries_before: usize,
    /// Number of entries after truncation.
    pub entries_after: usize,
    /// Whether truncation actually occurred.
    pub truncated: bool,
}

/// Session manager for named sessions and inter-session messaging.
pub struct SessionManager {
    sessions: HashMap<String, NamedSession>,
    active_session: Option<String>,
    message_bus: broadcast::Sender<InterSessionMessage>,
    inbox: HashMap<String, VecDeque<InterSessionMessage>>,
    sessions_dir: Option<PathBuf>,
    encryptor: SessionEncryptor,
}

impl SessionManager {
    /// Create a new session manager.
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel::<InterSessionMessage>(64);

        let sessions_dir = directories::ProjectDirs::from("ai", "nexibot", "desktop")
            .map(|dirs| dirs.config_dir().join("sessions"));

        let mut mgr = Self {
            sessions: HashMap::new(),
            active_session: None,
            message_bus: tx,
            inbox: HashMap::new(),
            sessions_dir: sessions_dir.clone(),
            encryptor: SessionEncryptor::disabled(),
        };

        // Load persisted sessions on startup
        if let Some(ref dir) = sessions_dir {
            mgr.load_persisted_sessions(dir);
        }

        mgr
    }

    /// Load existing session transcripts from disk.
    fn load_persisted_sessions(&mut self, dir: &PathBuf) {
        if !dir.exists() {
            return;
        }

        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) => {
                warn!("[SESSIONS] Failed to read sessions directory: {}", e);
                return;
            }
        };

        let mut count = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }

            let session_id = match path.file_stem().and_then(|s| s.to_str()) {
                Some(id) => id.to_string(),
                None => continue,
            };

            // Count lines to determine message count
            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            let mut name = session_id.clone();
            let mut created_at = Utc::now();
            let mut message_count = 0;
            let mut channel_type = default_channel_type();
            let mut decrypt_failures = 0u32;
            let mut total_lines = 0u32;

            for raw_line in content.lines() {
                if raw_line.trim().is_empty() {
                    continue;
                }
                total_lines += 1;
                let line = match self.encryptor.decrypt_line(raw_line) {
                    Ok(l) => l,
                    Err(_) => {
                        decrypt_failures += 1;
                        continue;
                    }
                };
                if let Ok(entry) = serde_json::from_str::<TranscriptEntry>(&line) {
                    if message_count == 0 {
                        created_at = entry.timestamp;
                    }
                    // Use first message's "from" as a hint for session name
                    if entry.entry_type == "session_created" {
                        name = entry.content.clone();
                        // Restore persisted channel_type (if present)
                        if let Some(ref ct) = entry.channel_type {
                            channel_type = ct.clone();
                        }
                    }
                    message_count += 1;
                }
            }

            // If all lines failed to decrypt, the encryption key likely changed
            if decrypt_failures > 0 && decrypt_failures == total_lines {
                warn!(
                    "[SESSION] Session '{}' has {} encrypted lines that cannot be decrypted \
                     (encryption key may have changed). Session data is inaccessible.",
                    session_id, total_lines
                );
                name = format!("{} (encrypted, key mismatch)", session_id);
            } else if decrypt_failures > 0 {
                warn!(
                    "[SESSION] Session '{}': {}/{} lines failed decryption",
                    session_id, decrypt_failures, total_lines
                );
            }

            let session = NamedSession {
                id: session_id.clone(),
                name,
                created_at,
                message_count,
                channel_type,
            };

            self.sessions.insert(session_id.clone(), session);
            self.inbox.insert(session_id, VecDeque::new());
            count += 1;
        }

        if count > 0 {
            info!("[SESSIONS] Loaded {} persisted sessions", count);
        }
    }

    /// Get the transcript file path for a session.
    fn transcript_path(&self, session_id: &str) -> Option<PathBuf> {
        self.sessions_dir
            .as_ref()
            .map(|dir| dir.join(format!("{}.jsonl", session_id)))
    }

    /// Persist a session creation to disk.
    fn persist_session_created(&self, session: &NamedSession) {
        let Some(path) = self.transcript_path(&session.id) else {
            return;
        };

        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                warn!("[SESSIONS] Failed to create sessions directory: {}", e);
                return;
            }
        }

        let entry = TranscriptEntry {
            entry_type: "session_created".to_string(),
            from: "system".to_string(),
            content: session.name.clone(),
            timestamp: session.created_at,
            channel_type: Some(session.channel_type.clone()),
        };

        if let Err(e) = self.append_entry_to_file(&path, &entry) {
            warn!("[SESSIONS] Failed to persist session creation: {}", e);
        }
    }

    /// Append a message to the session transcript on disk.
    pub fn append_message_to_transcript(&self, session_id: &str, from: &str, content: &str) {
        let Some(path) = self.transcript_path(session_id) else {
            return;
        };

        let entry = TranscriptEntry {
            entry_type: "message".to_string(),
            from: from.to_string(),
            content: content.to_string(),
            timestamp: Utc::now(),
            channel_type: None,
        };

        if let Err(e) = self.append_entry_to_file(&path, &entry) {
            warn!("[SESSIONS] Failed to append transcript: {}", e);
        }
    }

    /// Set the session encryptor (called after construction with config-derived settings).
    pub fn set_encryptor(&mut self, enc: SessionEncryptor) {
        self.encryptor = enc;
    }

    /// Append a JSONL entry to a file with restrictive permissions.
    fn append_entry_to_file(&self, path: &PathBuf, entry: &TranscriptEntry) -> Result<(), String> {
        let plaintext = serde_json::to_string(entry).map_err(|e| format!("Serialize error: {}", e))?;
        let line = match self.encryptor.encrypt_line(&plaintext) {
            Ok(enc) => enc,
            Err(e) => {
                warn!("[SESSIONS] Encrypt failed, writing plaintext: {}", e);
                plaintext
            }
        };

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| format!("File open error: {}", e))?;

        writeln!(file, "{}", line).map_err(|e| format!("Write error: {}", e))?;
        // Flush to OS to reduce data loss on crash
        let _ = file.sync_data();

        // Set restrictive permissions (cross-platform)
        let _ = crate::platform::file_security::restrict_file_permissions(path);

        Ok(())
    }

    /// Create a new named session.
    ///
    /// The optional `channel_type` identifies the channel that owns this session
    /// (e.g. `"gui"`, `"telegram"`, `"whatsapp"`). Defaults to `"gui"` when
    /// `None` is passed.
    pub fn create_session(&mut self, name: &str) -> Result<NamedSession, String> {
        self.create_session_with_channel(name, None)
    }

    /// Create a new named session with an explicit channel type.
    pub fn create_session_with_channel(
        &mut self,
        name: &str,
        channel_type: Option<&str>,
    ) -> Result<NamedSession, String> {
        if self.sessions.len() >= MAX_SESSIONS {
            return Err(format!(
                "Session limit reached ({}/{})",
                self.sessions.len(),
                MAX_SESSIONS
            ));
        }

        let id = uuid::Uuid::new_v4().to_string();
        if self.sessions.values().any(|s| s.name == name) {
            return Err(format!("Session with name '{}' already exists", name));
        }

        let session = NamedSession {
            id: id.clone(),
            name: name.to_string(),
            created_at: Utc::now(),
            message_count: 0,
            channel_type: channel_type.unwrap_or("gui").to_string(),
        };

        self.sessions.insert(id.clone(), session.clone());
        self.inbox.insert(id.clone(), VecDeque::new());

        if self.active_session.is_none() {
            self.active_session = Some(id);
        }

        // Persist to disk
        self.persist_session_created(&session);

        info!("[SESSIONS] Created session '{}' ({})", name, session.id);
        Ok(session)
    }

    /// List all sessions.
    pub fn list_sessions(&self) -> Vec<NamedSession> {
        self.sessions.values().cloned().collect()
    }

    /// Switch active session.
    pub fn switch_session(&mut self, id: &str) -> Result<(), String> {
        if !self.sessions.contains_key(id) {
            return Err(format!("Session '{}' not found", id));
        }
        self.active_session = Some(id.to_string());
        Ok(())
    }

    /// Get active session ID.
    #[allow(dead_code)]
    pub fn active_session_id(&self) -> Option<&str> {
        self.active_session.as_deref()
    }

    /// Send a message from one session to another.
    ///
    /// Validates:
    /// - Sender session ID exists
    /// - Target session exists (by name or ID)
    /// - Cross-channel messages (e.g. telegram -> gui) are wrapped with
    ///   content boundary markers via [`wrap_external_content`]
    /// - Final content length is bounded (max 10,000 chars), checked AFTER
    ///   the boundary marker is applied so marker overhead is accounted for
    pub fn send_to_session(&mut self, from: &str, to: &str, content: &str) -> Result<(), String> {
        // Validate sender session exists
        let from_channel = self
            .sessions
            .get(from)
            .map(|s| s.channel_type.clone())
            .ok_or_else(|| {
                warn!(
                    "[SESSIONS] Message rejected: sender session '{}' not found",
                    from,
                );
                format!("Sender session '{}' not found", from)
            })?;

        // Find target by name or ID
        let (to_id, to_channel) = self
            .sessions
            .iter()
            .find(|(id, s)| *id == to || s.name == to)
            .map(|(id, s)| (id.clone(), s.channel_type.clone()))
            .ok_or_else(|| {
                warn!(
                    "[SESSIONS] Message rejected: target session '{}' not found",
                    to,
                );
                format!("Target session '{}' not found", to)
            })?;

        // Apply cross-channel boundary marker when channel types differ
        let final_content = if from_channel != to_channel {
            info!(
                "[SESSIONS] Cross-channel message: {} ({}) -> {} ({}), applying boundary marker",
                from, from_channel, to_id, to_channel,
            );
            wrap_external_content(
                content,
                &format!("inter-session from {} (channel: {})", from, from_channel),
            )
        } else {
            content.to_string()
        };

        // Validate content length AFTER boundary marker is applied, so that
        // cross-channel marker overhead is accounted for in the limit.
        if final_content.len() > MAX_MESSAGE_LENGTH {
            warn!(
                "[SESSIONS] Message rejected: content length {} exceeds max {} (from={})",
                final_content.len(),
                MAX_MESSAGE_LENGTH,
                from,
            );
            return Err(format!(
                "Message content too long ({} chars, max {})",
                final_content.len(),
                MAX_MESSAGE_LENGTH,
            ));
        }

        let msg = InterSessionMessage {
            from_session: from.to_string(),
            to_session: to_id.clone(),
            content: final_content.clone(),
            timestamp: Utc::now(),
        };

        // Add to inbox, evicting oldest if over limit (O(1) via VecDeque)
        let inbox = self.inbox.entry(to_id.clone()).or_default();
        inbox.push_back(msg.clone());
        while inbox.len() > MAX_INBOX_SIZE {
            inbox.pop_front();
            warn!("[SESSION] Inbox for '{}' exceeded MAX_INBOX_SIZE ({}); oldest message evicted", to_id, MAX_INBOX_SIZE);
        }

        // Persist the message
        self.append_message_to_transcript(&to_id, from, &final_content);

        // Update message count
        if let Some(session) = self.sessions.get_mut(&to_id) {
            session.message_count += 1;
        }

        // Broadcast
        let _ = self.message_bus.send(msg);

        Ok(())
    }

    /// Get messages in a session's inbox.
    ///
    /// `caller_id` is the session ID of the entity requesting the inbox.
    /// When provided and it does not match `session_id`, the access is logged
    /// as a cross-session read for audit purposes.
    pub fn get_inbox(&self, session_id: &str) -> Vec<InterSessionMessage> {
        self.inbox.get(session_id)
            .map(|dq| dq.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Get messages in a session's inbox with caller tracking for audit logging.
    ///
    /// When `caller_id` differs from `session_id`, the cross-session read is
    /// logged as a warning so audit trails capture unexpected access patterns.
    #[allow(dead_code)]
    pub fn get_inbox_as(&self, session_id: &str, caller_id: &str) -> Vec<InterSessionMessage> {
        if caller_id != session_id {
            warn!(
                "[SESSION] Cross-session inbox read: caller '{}' reading inbox of session '{}'",
                caller_id, session_id
            );
        }
        self.get_inbox(session_id)
    }

    /// Consume and clear all messages in a session's inbox.
    #[allow(dead_code)]
    pub fn drain_inbox(&mut self, session_id: &str) -> Vec<InterSessionMessage> {
        self.inbox
            .get_mut(session_id)
            .map(|dq| std::mem::take(dq).into_iter().collect())
            .unwrap_or_default()
    }

    /// Consume and clear a session's inbox with caller tracking for audit logging.
    ///
    /// When `caller_id` differs from `session_id`, the cross-session drain is
    /// logged as a warning so audit trails capture unexpected access patterns.
    #[allow(dead_code)]
    pub fn drain_inbox_as(&mut self, session_id: &str, caller_id: &str) -> Vec<InterSessionMessage> {
        if caller_id != session_id {
            warn!(
                "[SESSION] Cross-session inbox drain: caller '{}' draining inbox of session '{}'",
                caller_id, session_id
            );
        }
        self.drain_inbox(session_id)
    }

    /// Subscribe to the message bus.
    #[allow(dead_code)]
    pub fn subscribe(&self) -> broadcast::Receiver<InterSessionMessage> {
        self.message_bus.subscribe()
    }

    /// Truncate a session transcript to the last `keep_entries` entries.
    ///
    /// If the transcript has fewer entries than `keep_entries`, this is a no-op.
    /// Uses file-level locking to prevent concurrent truncation races, plus
    /// atomic write (temp file + rename) to prevent data loss on crash.
    /// Prepends a `[TRUNCATED]` marker entry with the compaction summary.
    pub fn truncate_transcript(
        &self,
        session_id: &str,
        keep_entries: usize,
        compaction_summary: &str,
    ) -> Result<TruncationResult, String> {
        let path = self
            .transcript_path(session_id)
            .ok_or("No sessions directory configured")?;

        if !path.exists() {
            return Ok(TruncationResult {
                entries_before: 0,
                entries_after: 0,
                truncated: false,
            });
        }

        // Use a lock file to prevent concurrent truncation races.
        // Two concurrent truncations on the same session could both read N
        // entries, compute truncation, and overwrite each other's results.
        // We use OpenOptions::create_new as an atomic "lock acquire" — if the
        // file already exists, another truncation is in progress (or a prior
        // process crashed). Stale locks older than 60s are reclaimed.
        let lock_path = path.with_extension("jsonl.lock");
        let _lock_guard = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(f) => Some(f),
            Err(_) => {
                // Lock file exists — check if it's stale (older than 60 seconds)
                let is_stale = std::fs::metadata(&lock_path)
                    .and_then(|m| m.modified())
                    .map(|mtime| {
                        mtime.elapsed().map(|d| d.as_secs() > 60).unwrap_or(false)
                    })
                    .unwrap_or(true); // If we can't read metadata, treat as stale

                if is_stale {
                    warn!(
                        "[SESSIONS] Removing stale truncation lock for '{}'",
                        session_id
                    );
                    let _ = std::fs::remove_file(&lock_path);
                    // Try to acquire again
                    match std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&lock_path)
                    {
                        Ok(f) => Some(f),
                        Err(_) => {
                            warn!(
                                "[SESSIONS] Skipping truncation for '{}': cannot acquire lock",
                                session_id
                            );
                            return Ok(TruncationResult {
                                entries_before: 0,
                                entries_after: 0,
                                truncated: false,
                            });
                        }
                    }
                } else {
                    warn!(
                        "[SESSIONS] Skipping truncation for '{}': another truncation in progress",
                        session_id
                    );
                    return Ok(TruncationResult {
                        entries_before: 0,
                        entries_after: 0,
                        truncated: false,
                    });
                }
            }
        };
        // Ensure lock file is removed when we're done (even on error)
        struct LockGuard(std::path::PathBuf);
        impl Drop for LockGuard {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let _cleanup = LockGuard(lock_path);

        // Read all lines (under lock)
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read transcript: {}", e))?;

        let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
        let total = lines.len();

        if total <= keep_entries {
            return Ok(TruncationResult {
                entries_before: total,
                entries_after: total,
                truncated: false,
            });
        }

        info!(
            "[SESSIONS] Truncating session '{}': {} -> {} entries",
            session_id, total, keep_entries
        );

        // Build truncated content: marker + last N entries
        let marker = TranscriptEntry {
            entry_type: "truncated".to_string(),
            from: "system".to_string(),
            content: format!(
                "[TRUNCATED] {} entries removed during compaction. Summary: {}",
                total - keep_entries,
                compaction_summary
            ),
            timestamp: Utc::now(),
            channel_type: None,
        };

        let marker_json = serde_json::to_string(&marker)
            .map_err(|e| format!("Failed to serialize marker: {}", e))?;
        let marker_line = match self.encryptor.encrypt_line(&marker_json) {
            Ok(enc) => enc,
            Err(_) => marker_json,
        };

        let kept_lines = &lines[total - keep_entries..];

        // Atomic write: write to temp file, then rename
        let tmp_path = path.with_extension("jsonl.tmp");
        {
            let mut file = std::fs::File::create(&tmp_path)
                .map_err(|e| format!("Failed to create temp file: {}", e))?;
            writeln!(file, "{}", marker_line)
                .map_err(|e| format!("Failed to write marker: {}", e))?;
            for line in kept_lines {
                writeln!(file, "{}", line)
                    .map_err(|e| format!("Failed to write entry: {}", e))?;
            }
            file.sync_all()
                .map_err(|e| format!("Failed to sync temp file: {}", e))?;
        }

        // Rename (atomic on same filesystem)
        std::fs::rename(&tmp_path, &path)
            .map_err(|e| format!("Failed to rename temp file: {}", e))?;

        // Set restrictive permissions
        let _ = crate::platform::file_security::restrict_file_permissions(&path);

        let entries_after = keep_entries + 1; // +1 for the marker
        info!(
            "[SESSIONS] Truncation complete: {} -> {} entries",
            total, entries_after
        );

        Ok(TruncationResult {
            entries_before: total,
            entries_after,
            truncated: true,
        })
    }

    /// Delete a session.
    pub fn delete_session(&mut self, id: &str) -> Result<(), String> {
        if !self.sessions.contains_key(id) {
            return Err(format!("Session '{}' not found", id));
        }
        self.sessions.remove(id);
        self.inbox.remove(id);
        if self.active_session.as_deref() == Some(id) {
            self.active_session = self.sessions.keys().next().cloned();
        }
        // Note: transcript file is kept on disk for audit trail
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper to create a manager with temp dir for testing
    fn test_manager() -> SessionManager {
        let (tx, _) = broadcast::channel::<InterSessionMessage>(64);
        SessionManager {
            sessions: HashMap::new(),
            active_session: None,
            message_bus: tx,
            inbox: HashMap::new(),
            sessions_dir: None, // No disk persistence in tests
            encryptor: SessionEncryptor::disabled(),
        }
    }

    #[test]
    fn test_new_creates_empty_manager() {
        let mgr = test_manager();
        assert!(mgr.list_sessions().is_empty());
        assert!(mgr.active_session_id().is_none());
    }

    #[test]
    fn test_create_session_returns_named_session() {
        let mut mgr = test_manager();
        let session = mgr.create_session("Research").unwrap();
        assert_eq!(session.name, "Research");
        assert_eq!(session.message_count, 0);
    }

    #[test]
    fn test_first_created_session_auto_activates() {
        let mut mgr = test_manager();
        let session = mgr.create_session("First").unwrap();
        assert_eq!(mgr.active_session_id(), Some(session.id.as_str()));
    }

    #[test]
    fn test_creating_duplicate_name_returns_error() {
        let mut mgr = test_manager();
        mgr.create_session("Research").unwrap();
        let result = mgr.create_session("Research");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("already exists"));
    }

    #[test]
    fn test_list_sessions_returns_all() {
        let mut mgr = test_manager();
        mgr.create_session("A").unwrap();
        mgr.create_session("B").unwrap();
        mgr.create_session("C").unwrap();
        assert_eq!(mgr.list_sessions().len(), 3);
    }

    #[test]
    fn test_switch_session_valid_id() {
        let mut mgr = test_manager();
        let s1 = mgr.create_session("First").unwrap();
        let s2 = mgr.create_session("Second").unwrap();
        assert_eq!(mgr.active_session_id(), Some(s1.id.as_str()));

        mgr.switch_session(&s2.id).unwrap();
        assert_eq!(mgr.active_session_id(), Some(s2.id.as_str()));
    }

    #[test]
    fn test_switch_session_nonexistent_returns_error() {
        let mut mgr = test_manager();
        let result = mgr.switch_session("nonexistent");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[test]
    fn test_delete_session_removes_from_list() {
        let mut mgr = test_manager();
        let session = mgr.create_session("ToDelete").unwrap();
        assert_eq!(mgr.list_sessions().len(), 1);

        mgr.delete_session(&session.id).unwrap();
        assert!(mgr.list_sessions().is_empty());
    }

    #[test]
    fn test_delete_active_session_clears_active() {
        let mut mgr = test_manager();
        let session = mgr.create_session("Only").unwrap();
        assert_eq!(mgr.active_session_id(), Some(session.id.as_str()));

        mgr.delete_session(&session.id).unwrap();
        // With no sessions left, active should be None
        assert!(mgr.active_session_id().is_none());
    }

    #[test]
    fn test_delete_active_session_with_others_remaining() {
        let mut mgr = test_manager();
        let s1 = mgr.create_session("First").unwrap();
        let s2 = mgr.create_session("Second").unwrap();
        // s1 is active (first created)
        assert_eq!(mgr.active_session_id(), Some(s1.id.as_str()));

        mgr.delete_session(&s1.id).unwrap();
        // Active should switch to the remaining session
        assert_eq!(mgr.active_session_id(), Some(s2.id.as_str()));
    }

    #[test]
    fn test_delete_nonexistent_session_returns_error() {
        let mut mgr = test_manager();
        let result = mgr.delete_session("nonexistent");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[test]
    fn test_send_to_session_by_name() {
        let mut mgr = test_manager();
        let s1 = mgr.create_session("Sender").unwrap();
        let s2 = mgr.create_session("Receiver").unwrap();

        mgr.send_to_session(&s1.id, "Receiver", "Hello!").unwrap();

        let inbox = mgr.get_inbox(&s2.id);
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].content, "Hello!");
    }

    #[test]
    fn test_send_to_session_by_id() {
        let mut mgr = test_manager();
        let s1 = mgr.create_session("Sender").unwrap();
        let s2 = mgr.create_session("Receiver").unwrap();

        mgr.send_to_session(&s1.id, &s2.id, "Hello by ID!").unwrap();

        let inbox = mgr.get_inbox(&s2.id);
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].content, "Hello by ID!");
    }

    #[test]
    fn test_send_to_nonexistent_target_returns_error() {
        let mut mgr = test_manager();
        let s1 = mgr.create_session("Sender").unwrap();

        let result = mgr.send_to_session(&s1.id, "ghost", "Hello?");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[test]
    fn test_send_from_nonexistent_sender_returns_error() {
        let mut mgr = test_manager();
        mgr.create_session("Target").unwrap();

        let result = mgr.send_to_session("nonexistent-sender", "Target", "Hello?");
        assert!(result.is_err());
        let err_msg = result.unwrap_err();
        assert!(err_msg.contains("Sender session"));
        assert!(err_msg.contains("not found"));
    }

    #[test]
    fn test_get_inbox_returns_messages() {
        let mut mgr = test_manager();
        let s1 = mgr.create_session("A").unwrap();
        let s2 = mgr.create_session("B").unwrap();

        mgr.send_to_session(&s1.id, &s2.id, "msg1").unwrap();
        mgr.send_to_session(&s1.id, &s2.id, "msg2").unwrap();

        let inbox = mgr.get_inbox(&s2.id);
        assert_eq!(inbox.len(), 2);
        assert_eq!(inbox[0].content, "msg1");
        assert_eq!(inbox[1].content, "msg2");
    }

    #[test]
    fn test_get_inbox_empty_for_session_with_no_messages() {
        let mut mgr = test_manager();
        let session = mgr.create_session("Lonely").unwrap();
        let inbox = mgr.get_inbox(&session.id);
        assert!(inbox.is_empty());
    }

    #[test]
    fn test_session_ids_are_unique() {
        let mut mgr = test_manager();
        let s1 = mgr.create_session("A").unwrap();
        let s2 = mgr.create_session("B").unwrap();
        let s3 = mgr.create_session("C").unwrap();
        assert_ne!(s1.id, s2.id);
        assert_ne!(s2.id, s3.id);
        assert_ne!(s1.id, s3.id);
    }

    #[test]
    fn test_multiple_sessions_coexist() {
        let mut mgr = test_manager();
        mgr.create_session("Alpha").unwrap();
        mgr.create_session("Beta").unwrap();
        mgr.create_session("Gamma").unwrap();

        let sessions = mgr.list_sessions();
        assert_eq!(sessions.len(), 3);
        let names: Vec<&str> = sessions.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Alpha"));
        assert!(names.contains(&"Beta"));
        assert!(names.contains(&"Gamma"));
    }

    #[test]
    fn test_message_from_to_fields_populated() {
        let mut mgr = test_manager();
        let s1 = mgr.create_session("Source").unwrap();
        let s2 = mgr.create_session("Target").unwrap();

        mgr.send_to_session(&s1.id, &s2.id, "payload").unwrap();

        let inbox = mgr.get_inbox(&s2.id);
        assert_eq!(inbox[0].from_session, s1.id);
        assert_eq!(inbox[0].to_session, s2.id);
        assert_eq!(inbox[0].content, "payload");
    }

    #[test]
    fn test_active_session_id_none_initially_some_after_create() {
        let mut mgr = test_manager();
        assert!(mgr.active_session_id().is_none());

        let session = mgr.create_session("New").unwrap();
        assert_eq!(mgr.active_session_id(), Some(session.id.as_str()));
    }

    #[test]
    fn test_send_increments_message_count() {
        let mut mgr = test_manager();
        let s1 = mgr.create_session("A").unwrap();
        let s2 = mgr.create_session("B").unwrap();

        mgr.send_to_session(&s1.id, &s2.id, "msg1").unwrap();
        mgr.send_to_session(&s1.id, &s2.id, "msg2").unwrap();

        let sessions = mgr.list_sessions();
        let target = sessions.iter().find(|s| s.id == s2.id).unwrap();
        assert_eq!(target.message_count, 2);
    }

    #[test]
    fn test_inbox_bounded_at_max_size() {
        let mut mgr = test_manager();
        let s1 = mgr.create_session("Sender").unwrap();
        let s2 = mgr.create_session("Receiver").unwrap();

        // Send more messages than the inbox limit
        for i in 0..(MAX_INBOX_SIZE + 200) {
            mgr.send_to_session(&s1.id, &s2.id, &format!("msg-{}", i))
                .unwrap();
        }

        let inbox = mgr.get_inbox(&s2.id);
        assert_eq!(inbox.len(), MAX_INBOX_SIZE);
        // Oldest messages should have been drained; newest should be present
        assert_eq!(
            inbox.last().unwrap().content,
            format!("msg-{}", MAX_INBOX_SIZE + 199)
        );
    }

    #[test]
    fn test_session_limit_rejects_creation() {
        let mut mgr = test_manager();

        // Create MAX_SESSIONS sessions
        for i in 0..MAX_SESSIONS {
            mgr.create_session(&format!("session-{}", i)).unwrap();
        }
        assert_eq!(mgr.list_sessions().len(), MAX_SESSIONS);

        // Next creation should fail
        let result = mgr.create_session("one-too-many");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Session limit reached"));
    }

    #[test]
    fn test_drain_inbox_returns_and_clears() {
        let mut mgr = test_manager();
        let s1 = mgr.create_session("A").unwrap();
        let s2 = mgr.create_session("B").unwrap();

        mgr.send_to_session(&s1.id, &s2.id, "msg1").unwrap();
        mgr.send_to_session(&s1.id, &s2.id, "msg2").unwrap();

        let drained = mgr.drain_inbox(&s2.id);
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].content, "msg1");

        // Inbox should now be empty
        let inbox = mgr.get_inbox(&s2.id);
        assert!(inbox.is_empty());
    }

    #[test]
    #[ignore] // Benchmark — run with: cargo test benchmark_inbox -- --ignored --test-threads=1
    fn benchmark_inbox_bounded_insert() {
        use std::time::Instant;

        let mut mgr = test_manager();
        let s1 = mgr.create_session("Sender").unwrap();
        let s2 = mgr.create_session("Receiver").unwrap();

        let msg_count = 5000;
        let start = Instant::now();

        for i in 0..msg_count {
            mgr.send_to_session(&s1.id, &s2.id, &format!("bench-msg-{}", i))
                .unwrap();
        }

        let elapsed = start.elapsed();
        let inbox = mgr.get_inbox(&s2.id);

        eprintln!(
            "[BENCHMARK] {} messages sent in {:?} ({:.1}us/msg), inbox.len() = {}",
            msg_count,
            elapsed,
            elapsed.as_micros() as f64 / msg_count as f64,
            inbox.len()
        );

        // Verify inbox is bounded
        assert!(
            inbox.len() <= MAX_INBOX_SIZE,
            "Inbox exceeded limit: {} > {}",
            inbox.len(),
            MAX_INBOX_SIZE
        );
    }

    // -----------------------------------------------------------------------
    // Cross-context messaging policy tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_message_content_length_limit() {
        let mut mgr = test_manager();
        let s1 = mgr.create_session("Sender").unwrap();
        let s2 = mgr.create_session("Receiver").unwrap();

        // Exactly at limit should succeed
        let at_limit = "x".repeat(MAX_MESSAGE_LENGTH);
        assert!(mgr.send_to_session(&s1.id, &s2.id, &at_limit).is_ok());

        // Over limit should fail
        let over_limit = "x".repeat(MAX_MESSAGE_LENGTH + 1);
        let result = mgr.send_to_session(&s1.id, &s2.id, &over_limit);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("too long"));
    }

    #[test]
    fn test_same_channel_no_boundary_marker() {
        let mut mgr = test_manager();
        let s1 = mgr
            .create_session_with_channel("GuiA", Some("gui"))
            .unwrap();
        let s2 = mgr
            .create_session_with_channel("GuiB", Some("gui"))
            .unwrap();

        mgr.send_to_session(&s1.id, &s2.id, "same channel msg")
            .unwrap();

        let inbox = mgr.get_inbox(&s2.id);
        assert_eq!(inbox.len(), 1);
        // Same channel: content should NOT be wrapped with boundary markers
        assert_eq!(inbox[0].content, "same channel msg");
        assert!(!inbox[0].content.contains("EXTERNAL_UNTRUSTED_CONTENT"));
    }

    #[test]
    fn test_cross_channel_adds_boundary_marker() {
        let mut mgr = test_manager();
        let s1 = mgr
            .create_session_with_channel("TgSession", Some("telegram"))
            .unwrap();
        let s2 = mgr
            .create_session_with_channel("GuiSession", Some("gui"))
            .unwrap();

        mgr.send_to_session(&s1.id, &s2.id, "cross channel msg")
            .unwrap();

        let inbox = mgr.get_inbox(&s2.id);
        assert_eq!(inbox.len(), 1);
        // Cross-channel: content should be wrapped with boundary markers
        assert!(inbox[0].content.contains("EXTERNAL_UNTRUSTED_CONTENT"));
        assert!(inbox[0].content.contains("cross channel msg"));
        assert!(inbox[0].content.contains("telegram"));
    }

    #[test]
    fn test_create_session_with_channel_type() {
        let mut mgr = test_manager();
        let session = mgr
            .create_session_with_channel("TgTest", Some("telegram"))
            .unwrap();
        assert_eq!(session.channel_type, "telegram");

        let default_session = mgr.create_session("DefaultChannel").unwrap();
        assert_eq!(default_session.channel_type, "gui");
    }

    #[test]
    fn test_cross_channel_at_limit_rejected_with_marker_overhead() {
        let mut mgr = test_manager();
        let s1 = mgr
            .create_session_with_channel("TgSender", Some("telegram"))
            .unwrap();
        let s2 = mgr
            .create_session_with_channel("GuiReceiver", Some("gui"))
            .unwrap();

        // A message exactly at MAX_MESSAGE_LENGTH should be rejected for
        // cross-channel sends because the boundary marker pushes the final
        // content over the limit.
        let at_limit = "x".repeat(MAX_MESSAGE_LENGTH);
        let result = mgr.send_to_session(&s1.id, &s2.id, &at_limit);
        assert!(result.is_err(), "Cross-channel message at raw limit should be rejected due to marker overhead");
        assert!(result.unwrap_err().contains("too long"));
    }

    #[test]
    fn test_same_channel_at_limit_still_accepted() {
        let mut mgr = test_manager();
        let s1 = mgr
            .create_session_with_channel("GuiA", Some("gui"))
            .unwrap();
        let s2 = mgr
            .create_session_with_channel("GuiB", Some("gui"))
            .unwrap();

        // Same-channel message exactly at the limit should still succeed
        // (no boundary marker overhead).
        let at_limit = "x".repeat(MAX_MESSAGE_LENGTH);
        assert!(mgr.send_to_session(&s1.id, &s2.id, &at_limit).is_ok());
    }

    #[test]
    fn test_cross_channel_whatsapp_to_telegram() {
        let mut mgr = test_manager();
        let s1 = mgr
            .create_session_with_channel("WA", Some("whatsapp"))
            .unwrap();
        let s2 = mgr
            .create_session_with_channel("TG", Some("telegram"))
            .unwrap();

        mgr.send_to_session(&s1.id, &s2.id, "wa -> tg").unwrap();

        let inbox = mgr.get_inbox(&s2.id);
        assert!(inbox[0].content.contains("EXTERNAL_UNTRUSTED_CONTENT"));
        assert!(inbox[0].content.contains("whatsapp"));
    }

    // -----------------------------------------------------------------------
    // Transcript truncation tests
    // -----------------------------------------------------------------------

    /// Helper to create a manager with a real temp directory for truncation tests.
    fn test_manager_with_dir(dir: &std::path::Path) -> SessionManager {
        let (tx, _) = broadcast::channel::<InterSessionMessage>(64);
        SessionManager {
            sessions: HashMap::new(),
            active_session: None,
            message_bus: tx,
            inbox: HashMap::new(),
            sessions_dir: Some(dir.to_path_buf()),
            encryptor: SessionEncryptor::disabled(),
        }
    }

    /// Write N JSONL lines to a transcript file for testing.
    fn write_test_transcript(path: &std::path::Path, n: usize) {
        let mut file = std::fs::File::create(path).unwrap();
        for i in 0..n {
            let entry = TranscriptEntry {
                entry_type: "message".to_string(),
                from: "user".to_string(),
                content: format!("message-{}", i),
                timestamp: Utc::now(),
                channel_type: None,
            };
            let json = serde_json::to_string(&entry).unwrap();
            writeln!(file, "{}", json).unwrap();
        }
    }

    #[test]
    fn test_truncate_transcript_no_op_when_below_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = test_manager_with_dir(tmp.path());

        let session_id = "test-session";
        let path = tmp.path().join(format!("{}.jsonl", session_id));
        write_test_transcript(&path, 50);

        let result = mgr.truncate_transcript(session_id, 200, "summary").unwrap();
        assert!(!result.truncated);
        assert_eq!(result.entries_before, 50);
        assert_eq!(result.entries_after, 50);

        // File should be unchanged
        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(lines.len(), 50);
    }

    #[test]
    fn test_truncate_transcript_truncates_to_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = test_manager_with_dir(tmp.path());

        let session_id = "test-session";
        let path = tmp.path().join(format!("{}.jsonl", session_id));
        write_test_transcript(&path, 300);

        let result = mgr.truncate_transcript(session_id, 100, "test summary").unwrap();
        assert!(result.truncated);
        assert_eq!(result.entries_before, 300);
        assert_eq!(result.entries_after, 101); // 100 kept + 1 marker

        // Verify file contents
        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(lines.len(), 101);

        // First line should be the truncation marker
        let marker: TranscriptEntry = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(marker.entry_type, "truncated");
        assert!(marker.content.contains("[TRUNCATED]"));
        assert!(marker.content.contains("200 entries removed"));
        assert!(marker.content.contains("test summary"));

        // Last entry should be the last message from the original file
        let last: TranscriptEntry = serde_json::from_str(lines[100]).unwrap();
        assert_eq!(last.content, "message-299");
    }

    #[test]
    fn test_truncate_transcript_nonexistent_file() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = test_manager_with_dir(tmp.path());

        let result = mgr.truncate_transcript("nonexistent", 100, "summary").unwrap();
        assert!(!result.truncated);
        assert_eq!(result.entries_before, 0);
        assert_eq!(result.entries_after, 0);
    }

    #[test]
    fn test_truncate_transcript_no_sessions_dir() {
        let mgr = test_manager(); // sessions_dir is None
        let result = mgr.truncate_transcript("any", 100, "summary");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No sessions directory"));
    }

    #[test]
    fn test_truncate_transcript_preserves_newest_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = test_manager_with_dir(tmp.path());

        let session_id = "test-session";
        let path = tmp.path().join(format!("{}.jsonl", session_id));
        write_test_transcript(&path, 10);

        let result = mgr.truncate_transcript(session_id, 5, "compact").unwrap();
        assert!(result.truncated);
        assert_eq!(result.entries_before, 10);
        assert_eq!(result.entries_after, 6); // 5 kept + 1 marker

        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();

        // Verify the kept entries are the last 5 (messages 5-9)
        for i in 1..=5 {
            let entry: TranscriptEntry = serde_json::from_str(lines[i]).unwrap();
            assert_eq!(entry.content, format!("message-{}", i + 4));
        }
    }

    #[test]
    fn test_truncate_transcript_exact_at_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = test_manager_with_dir(tmp.path());

        let session_id = "test-session";
        let path = tmp.path().join(format!("{}.jsonl", session_id));
        write_test_transcript(&path, 200);

        let result = mgr.truncate_transcript(session_id, 200, "summary").unwrap();
        assert!(!result.truncated);
        assert_eq!(result.entries_before, 200);
        assert_eq!(result.entries_after, 200);
    }

    #[test]
    fn test_truncate_transcript_temp_file_cleaned_up() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = test_manager_with_dir(tmp.path());

        let session_id = "test-session";
        let path = tmp.path().join(format!("{}.jsonl", session_id));
        let tmp_path = tmp.path().join(format!("{}.jsonl.tmp", session_id));
        write_test_transcript(&path, 50);

        mgr.truncate_transcript(session_id, 10, "summary").unwrap();

        // Temp file should not exist after successful truncation
        assert!(!tmp_path.exists());
        // Original file should still exist with truncated content
        assert!(path.exists());
    }
}
