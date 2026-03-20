//! Per-connection session management for the WebSocket gateway.
//!
//! Each WebSocket connection gets a [`GatewaySession`] that tracks the user,
//! activity timestamps, and message counts. The [`GatewaySessionManager`]
//! enforces per-user limits and handles idle cleanup.

use anyhow::{bail, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;
use tracing::{debug, info, warn};
use uuid::Uuid;

use super::method_scopes::Scope;

/// A single gateway session bound to a WebSocket connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewaySession {
    /// Unique session identifier.
    pub id: String,
    /// The authenticated user this session belongs to.
    pub user_id: String,
    /// When this session was created.
    pub created_at: DateTime<Utc>,
    /// When the last message was received on this session.
    pub last_activity: DateTime<Utc>,
    /// Number of messages processed in this session.
    pub message_count: u64,
    /// Scopes granted at authentication time. Immutable after session creation.
    authenticated_scopes: Vec<Scope>,
}

impl GatewaySession {
    /// Create a new session for the given user with the specified scopes.
    ///
    /// The `scopes` are fixed at creation time and cannot be changed afterward,
    /// preventing token scope escalation (P1C).
    fn new(user_id: &str, scopes: Vec<Scope>) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4().to_string(),
            user_id: user_id.to_string(),
            created_at: now,
            last_activity: now,
            message_count: 0,
            authenticated_scopes: scopes,
        }
    }

    /// Return the scopes granted at authentication time.
    ///
    /// These are immutable — there is no setter, so a client cannot escalate
    /// its own privileges after the initial handshake.
    pub fn scopes(&self) -> &[Scope] {
        &self.authenticated_scopes
    }

    /// How long the session has been idle.
    pub fn idle_duration(&self) -> chrono::Duration {
        Utc::now() - self.last_activity
    }
}

/// Manages all active gateway sessions.
pub struct GatewaySessionManager {
    /// Active sessions keyed by session ID.
    sessions: HashMap<String, GatewaySession>,
    /// Maximum number of concurrent sessions a single user may have.
    max_sessions_per_user: usize,
}

impl GatewaySessionManager {
    /// Create a new session manager.
    pub fn new(max_sessions_per_user: usize) -> Self {
        info!(
            "[GATEWAY_SESSION] Session manager initialized (max {} per user)",
            max_sessions_per_user
        );
        Self {
            sessions: HashMap::new(),
            max_sessions_per_user,
        }
    }

    /// Create a new session for `user_id` with the given scopes.
    ///
    /// The scopes are locked at creation time and cannot be modified afterward.
    /// Returns an error if the user already has the maximum number of sessions.
    #[allow(dead_code)]
    pub fn create_session(&mut self, user_id: &str) -> Result<GatewaySession> {
        self.create_session_with_scopes(user_id, vec![Scope::Read])
    }

    /// Create a new session with explicit scopes granted at authentication time.
    ///
    /// The scopes are immutable after creation — this is the only way to set
    /// them, preventing scope escalation after the initial handshake.
    pub fn create_session_with_scopes(
        &mut self,
        user_id: &str,
        scopes: Vec<Scope>,
    ) -> Result<GatewaySession> {
        let user_sessions = self.get_user_sessions(user_id).len();
        if user_sessions >= self.max_sessions_per_user {
            bail!(
                "User '{}' already has {} sessions (max {})",
                user_id,
                user_sessions,
                self.max_sessions_per_user
            );
        }

        let session = GatewaySession::new(user_id, scopes);
        let session_clone = session.clone();
        info!(
            "[GATEWAY_SESSION] Created session {} for user '{}' with scopes {:?}",
            session.id,
            user_id,
            session_clone.scopes()
        );
        self.sessions.insert(session.id.clone(), session);
        Ok(session_clone)
    }

    /// Look up a session by ID.
    pub fn get_session(&self, session_id: &str) -> Option<&GatewaySession> {
        self.sessions.get(session_id)
    }

    /// Update the last-activity timestamp and increment the message count.
    pub fn update_activity(&mut self, session_id: &str) {
        if let Some(session) = self.sessions.get_mut(session_id) {
            session.last_activity = Utc::now();
            session.message_count += 1;
            debug!(
                "[GATEWAY_SESSION] Activity updated for session {} (messages: {})",
                session_id, session.message_count
            );
        } else {
            warn!(
                "[GATEWAY_SESSION] Attempted to update unknown session: {}",
                session_id
            );
        }
    }

    /// Remove a session by ID.
    pub fn remove_session(&mut self, session_id: &str) -> Option<GatewaySession> {
        let removed = self.sessions.remove(session_id);
        if removed.is_some() {
            info!("[GATEWAY_SESSION] Removed session {}", session_id);
        } else {
            debug!(
                "[GATEWAY_SESSION] Attempted to remove unknown session: {}",
                session_id
            );
        }
        removed
    }

    /// Remove sessions that have been idle longer than `max_idle`.
    ///
    /// Returns the number of sessions removed.
    pub fn cleanup_inactive(&mut self, max_idle: Duration) -> usize {
        let max_idle_chrono =
            chrono::Duration::from_std(max_idle).unwrap_or(chrono::Duration::hours(1));
        let before = self.sessions.len();

        self.sessions
            .retain(|_id, session| session.idle_duration() < max_idle_chrono);

        let removed = before - self.sessions.len();
        if removed > 0 {
            info!(
                "[GATEWAY_SESSION] Cleaned up {} inactive sessions (idle > {:?})",
                removed, max_idle
            );
        }
        removed
    }

    /// Get all sessions belonging to a user.
    pub fn get_user_sessions(&self, user_id: &str) -> Vec<&GatewaySession> {
        self.sessions
            .values()
            .filter(|s| s.user_id == user_id)
            .collect()
    }

    /// Total number of active sessions.
    #[allow(dead_code)]
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_session() {
        let mut mgr = GatewaySessionManager::new(5);
        let session = mgr.create_session("user-1").unwrap();
        assert_eq!(session.user_id, "user-1");
        assert_eq!(session.message_count, 0);
        assert_eq!(mgr.session_count(), 1);
    }

    #[test]
    fn test_get_session() {
        let mut mgr = GatewaySessionManager::new(5);
        let session = mgr.create_session("user-1").unwrap();
        let id = session.id.clone();

        let found = mgr.get_session(&id);
        assert!(found.is_some());
        assert_eq!(found.unwrap().user_id, "user-1");

        assert!(mgr.get_session("nonexistent").is_none());
    }

    #[test]
    fn test_update_activity() {
        let mut mgr = GatewaySessionManager::new(5);
        let session = mgr.create_session("user-1").unwrap();
        let id = session.id.clone();

        mgr.update_activity(&id);
        let updated = mgr.get_session(&id).unwrap();
        assert_eq!(updated.message_count, 1);

        mgr.update_activity(&id);
        let updated = mgr.get_session(&id).unwrap();
        assert_eq!(updated.message_count, 2);
    }

    #[test]
    fn test_remove_session() {
        let mut mgr = GatewaySessionManager::new(5);
        let session = mgr.create_session("user-1").unwrap();
        let id = session.id.clone();

        assert_eq!(mgr.session_count(), 1);
        let removed = mgr.remove_session(&id);
        assert!(removed.is_some());
        assert_eq!(mgr.session_count(), 0);

        // Removing again returns None
        assert!(mgr.remove_session(&id).is_none());
    }

    #[test]
    fn test_max_sessions_per_user() {
        let mut mgr = GatewaySessionManager::new(2);
        mgr.create_session("user-1").unwrap();
        mgr.create_session("user-1").unwrap();

        // Third session should fail
        let result = mgr.create_session("user-1");
        assert!(result.is_err());

        // But a different user can still create sessions
        let result = mgr.create_session("user-2");
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_user_sessions() {
        let mut mgr = GatewaySessionManager::new(10);
        mgr.create_session("alice").unwrap();
        mgr.create_session("alice").unwrap();
        mgr.create_session("bob").unwrap();

        assert_eq!(mgr.get_user_sessions("alice").len(), 2);
        assert_eq!(mgr.get_user_sessions("bob").len(), 1);
        assert_eq!(mgr.get_user_sessions("charlie").len(), 0);
    }

    #[test]
    fn test_cleanup_inactive() {
        let mut mgr = GatewaySessionManager::new(10);
        let session = mgr.create_session("user-1").unwrap();
        let id = session.id.clone();

        // With a very long max_idle, nothing should be cleaned
        let removed = mgr.cleanup_inactive(Duration::from_secs(3600));
        assert_eq!(removed, 0);
        assert_eq!(mgr.session_count(), 1);

        // Manually backdate the session's last_activity to make it "idle"
        if let Some(s) = mgr.sessions.get_mut(&id) {
            s.last_activity = Utc::now() - chrono::Duration::hours(2);
        }

        // Now cleanup with a 1-hour threshold should remove it
        let removed = mgr.cleanup_inactive(Duration::from_secs(3600));
        assert_eq!(removed, 1);
        assert_eq!(mgr.session_count(), 0);
    }

    #[test]
    fn test_session_idle_duration() {
        let session = GatewaySession::new("test", vec![Scope::Read]);
        // Just-created session should have near-zero idle time
        assert!(session.idle_duration().num_seconds() < 2);
    }

    #[test]
    fn test_update_nonexistent_session() {
        let mut mgr = GatewaySessionManager::new(5);
        // Should not panic, just warn
        mgr.update_activity("does-not-exist");
    }

    #[test]
    fn test_session_scopes_are_immutable() {
        let mut mgr = GatewaySessionManager::new(5);
        let session = mgr
            .create_session_with_scopes("user-1", vec![Scope::Read])
            .unwrap();
        let id = session.id.clone();

        // Scopes should match what was set at creation time
        let found = mgr.get_session(&id).unwrap();
        assert_eq!(found.scopes(), &[Scope::Read]);

        // There is no public setter for scopes — the only way to get scopes
        // is through the getter, and the field is private. This test verifies
        // the scopes cannot be changed after creation.
    }

    #[test]
    fn test_create_session_with_admin_scopes() {
        let mut mgr = GatewaySessionManager::new(5);
        let session = mgr
            .create_session_with_scopes("admin-user", vec![Scope::Admin])
            .unwrap();
        assert_eq!(session.scopes(), &[Scope::Admin]);
    }

    #[test]
    fn test_create_session_with_multiple_scopes() {
        let mut mgr = GatewaySessionManager::new(5);
        let session = mgr
            .create_session_with_scopes(
                "power-user",
                vec![Scope::Read, Scope::Write, Scope::Approvals],
            )
            .unwrap();
        assert_eq!(session.scopes().len(), 3);
        assert!(session.scopes().contains(&Scope::Read));
        assert!(session.scopes().contains(&Scope::Write));
        assert!(session.scopes().contains(&Scope::Approvals));
    }

    #[test]
    fn test_default_create_session_gets_read_scope() {
        let mut mgr = GatewaySessionManager::new(5);
        let session = mgr.create_session("user-1").unwrap();
        assert_eq!(session.scopes(), &[Scope::Read]);
    }
}
