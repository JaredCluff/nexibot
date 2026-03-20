//! Multi-User and Family Mode Support
//!
//! Enables NexiBot to support multiple users with:
//! - Per-user memory isolation
//! - Shared memory pools (family/group level)
//! - Role-Based Access Control (RBAC)
//! - User invitation system
//! - Family/group management
//! - Activity logs per user

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

/// User roles in a family/group
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UserRole {
    /// Family admin - full control
    Admin,
    /// Parent/guardian - can manage other users
    Parent,
    /// Regular user - can use their own features
    User,
    /// Guest - read-only access
    Guest,
}

impl std::fmt::Display for UserRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UserRole::Admin => write!(f, "admin"),
            UserRole::Parent => write!(f, "parent"),
            UserRole::User => write!(f, "user"),
            UserRole::Guest => write!(f, "guest"),
        }
    }
}

impl UserRole {
    /// Check if role can invite users
    pub fn can_invite(&self) -> bool {
        matches!(self, UserRole::Admin | UserRole::Parent)
    }

    /// Check if role can manage other users
    #[allow(dead_code)]
    pub fn can_manage_users(&self) -> bool {
        matches!(self, UserRole::Admin | UserRole::Parent)
    }

    /// Check if role can view other users' activity
    #[allow(dead_code)]
    pub fn can_view_activity(&self) -> bool {
        matches!(self, UserRole::Admin | UserRole::Parent)
    }

    /// Check if role can access shared memory
    #[allow(dead_code)]
    pub fn can_access_shared_memory(&self) -> bool {
        true // All roles can access shared memory
    }
}

/// User in a family/group
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FamilyUser {
    /// User ID (UUID)
    pub id: String,

    /// Display name
    pub name: String,

    /// Email (optional)
    pub email: Option<String>,

    /// Avatar URL (optional)
    pub avatar_url: Option<String>,

    /// Role in the family
    pub role: UserRole,

    /// When user was added to family
    pub created_at: DateTime<Utc>,

    /// When user was last active
    pub last_active: DateTime<Utc>,

    /// Is this user active (enabled)
    pub is_active: bool,

    /// Preferences (JSON)
    pub preferences: serde_json::Value,
}

/// Family or group
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Family {
    /// Family ID (UUID)
    pub id: String,

    /// Family name
    pub name: String,

    /// Family description
    pub description: Option<String>,

    /// Admin user ID
    pub admin_id: String,

    /// All users in family (id -> user)
    pub users: HashMap<String, FamilyUser>,

    /// Shared memory pool IDs
    pub shared_memory_pool_ids: Vec<String>,

    /// When family was created
    pub created_at: DateTime<Utc>,

    /// Maximum users allowed (0 = unlimited)
    pub max_users: usize,

    /// Settings (JSON)
    pub settings: serde_json::Value,
}

/// Invitation to join a family
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invitation {
    /// Invitation ID
    pub id: String,

    /// Family ID
    pub family_id: String,

    /// Email address being invited
    pub email: String,

    /// Role for invited user
    pub role: UserRole,

    /// Invitation code (for email links)
    pub code: String,

    /// When invitation was sent
    pub created_at: DateTime<Utc>,

    /// When invitation expires
    pub expires_at: DateTime<Utc>,

    /// Is this invitation accepted
    pub accepted: bool,

    /// When invitation was accepted (if applicable)
    pub accepted_at: Option<DateTime<Utc>>,
}

/// Memory sharing configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySharing {
    /// ID of shared memory pool
    pub pool_id: String,

    /// Users who can access (empty = all)
    pub allowed_users: HashSet<String>,

    /// Access level
    pub access_level: MemoryAccessLevel,

    /// Created at
    pub created_at: DateTime<Utc>,
}

/// Memory access levels
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryAccessLevel {
    /// Read-only
    Read,
    /// Can read and write
    Write,
    /// Full control (can delete, share)
    Admin,
}

/// Activity log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityLogEntry {
    pub timestamp: DateTime<Utc>,
    pub user_id: String,
    pub action: String,
    pub details: String,
    pub ip_address: Option<String>,
}

/// Multi-user / Family Mode Manager
pub struct FamilyModeManager {
    families: Arc<RwLock<HashMap<String, Family>>>,
    invitations: Arc<RwLock<Vec<Invitation>>>,
    memory_sharing: Arc<RwLock<HashMap<String, MemorySharing>>>,
    activity_log: Arc<RwLock<Vec<ActivityLogEntry>>>,
}

impl FamilyModeManager {
    /// Create a new family mode manager
    pub fn new() -> Self {
        Self {
            families: Arc::new(RwLock::new(HashMap::new())),
            invitations: Arc::new(RwLock::new(Vec::new())),
            memory_sharing: Arc::new(RwLock::new(HashMap::new())),
            activity_log: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Create a new family
    pub async fn create_family(
        &self,
        admin_id: String,
        name: String,
        description: Option<String>,
    ) -> Result<String, String> {
        let family_id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();

        let admin_user = FamilyUser {
            id: admin_id.clone(),
            name: "Admin".to_string(),
            email: None,
            avatar_url: None,
            role: UserRole::Admin,
            created_at: now,
            last_active: now,
            is_active: true,
            preferences: serde_json::json!({}),
        };

        let family = Family {
            id: family_id.clone(),
            name,
            description,
            admin_id,
            users: vec![(admin_user.id.clone(), admin_user)]
                .into_iter()
                .collect(),
            shared_memory_pool_ids: Vec::new(),
            created_at: now,
            max_users: 10, // Default limit
            settings: serde_json::json!({}),
        };

        let mut families = self.families.write().await;
        families.insert(family_id.clone(), family);

        info!("[FAMILY] Created family: {}", family_id);
        Ok(family_id)
    }

    /// Get family by ID
    pub async fn get_family(&self, family_id: &str) -> Result<Family, String> {
        let families = self.families.read().await;
        families
            .get(family_id)
            .cloned()
            .ok_or_else(|| format!("Family not found: {}", family_id))
    }

    /// List all families for a user
    pub async fn list_user_families(&self, user_id: &str) -> Vec<Family> {
        let families = self.families.read().await;
        families
            .values()
            .filter(|f| f.users.contains_key(user_id))
            .cloned()
            .collect()
    }

    /// Send family invitation.
    ///
    /// Only Admin or Parent roles are permitted to send invitations.
    /// `caller_id` must be an existing member of the family with an
    /// appropriate role.
    pub async fn send_invitation(
        &self,
        family_id: &str,
        caller_id: &str,
        email: String,
        role: UserRole,
    ) -> Result<String, String> {
        // Verify family exists and caller has permission to invite.
        let families = self.families.read().await;
        let family = families
            .get(family_id)
            .ok_or_else(|| format!("Family not found: {}", family_id))?;

        let caller = family
            .users
            .get(caller_id)
            .ok_or_else(|| "Caller is not a member of this family".to_string())?;

        if !caller.role.can_invite() {
            return Err(format!(
                "Permission denied: role '{}' cannot send invitations (requires Admin or Parent)",
                caller.role
            ));
        }

        // Parents may only invite users with roles below Parent (they cannot
        // create other Admins or Parents).
        if caller.role == UserRole::Parent
            && matches!(role, UserRole::Admin | UserRole::Parent)
        {
            return Err(
                "Permission denied: Parents may only invite User or Guest roles".to_string(),
            );
        }

        drop(families);

        let invitation_id = uuid::Uuid::new_v4().to_string();
        // Use the full UUID string (128 bits) to prevent brute-force of the
        // invitation code. The earlier 12-char truncation gave only ~48 bits.
        let code = uuid::Uuid::new_v4().to_string();

        let invitation = Invitation {
            id: invitation_id.clone(),
            family_id: family_id.to_string(),
            email,
            role,
            code,
            created_at: Utc::now(),
            expires_at: Utc::now() + Duration::days(7),
            accepted: false,
            accepted_at: None,
        };

        let mut invitations = self.invitations.write().await;
        invitations.push(invitation);

        info!("[FAMILY] Sent invitation for family: {}", family_id);
        Ok(invitation_id)
    }

    /// Accept invitation.
    ///
    /// `invitation_code` is the short code embedded in the invitation link.
    /// `user_email` must match the email the invitation was sent to, preventing
    /// one user from consuming an invitation intended for another.
    pub async fn accept_invitation(
        &self,
        invitation_id: &str,
        invitation_code: &str,
        user_id: String,
        user_name: String,
        user_email: &str,
    ) -> Result<String, String> {
        let mut invitations = self.invitations.write().await;

        if let Some(invitation) = invitations.iter_mut().find(|i| i.id == invitation_id) {
            // Verify the short code matches (prevents enumeration of invitation IDs).
            if invitation.code != invitation_code {
                return Err("Invalid invitation code".to_string());
            }

            // Verify the invitation was issued to this specific user.
            if invitation.email != user_email {
                return Err(
                    "This invitation was not issued to your email address".to_string(),
                );
            }

            if invitation.accepted {
                return Err("Invitation already accepted".to_string());
            }

            if invitation.expires_at < Utc::now() {
                return Err("Invitation expired".to_string());
            }

            let family_id = invitation.family_id.clone();
            let role = invitation.role.clone();

            invitation.accepted = true;
            invitation.accepted_at = Some(Utc::now());

            // Add user to family
            let mut families = self.families.write().await;
            if let Some(family) = families.get_mut(&family_id) {
                let user = FamilyUser {
                    id: user_id.clone(),
                    name: user_name,
                    email: Some(invitation.email.clone()),
                    avatar_url: None,
                    role,
                    created_at: Utc::now(),
                    last_active: Utc::now(),
                    is_active: true,
                    preferences: serde_json::json!({}),
                };

                family.users.insert(user_id.clone(), user);
                info!("[FAMILY] User {} joined family {}", user_id, family_id);
                return Ok(family_id);
            }

            return Err("Family not found".to_string());
        }

        Err("Invitation not found".to_string())
    }

    /// Remove user from family.
    ///
    /// Authorization rules:
    /// - Only Admin can remove any non-admin member.
    /// - A Parent may only remove Child/Supervised users (User/Guest roles);
    ///   they cannot remove other Admins or Parents.
    /// - Nobody can remove the family Admin via this method.
    pub async fn remove_user(
        &self,
        family_id: &str,
        caller_id: &str,
        user_id: &str,
    ) -> Result<(), String> {
        let mut families = self.families.write().await;

        if let Some(family) = families.get_mut(family_id) {
            // Verify caller is a member.
            let caller_role = family
                .users
                .get(caller_id)
                .map(|u| u.role.clone())
                .ok_or_else(|| "Caller is not a member of this family".to_string())?;

            // Nobody may remove the admin.
            if family.admin_id == user_id {
                return Err("Cannot remove the family Admin".to_string());
            }

            // Determine the target's role for further checks.
            let target_role = family
                .users
                .get(user_id)
                .map(|u| u.role.clone())
                .ok_or_else(|| "User not found in family".to_string())?;

            match caller_role {
                UserRole::Admin => {
                    // Admins may remove any non-admin member (admin guard above).
                }
                UserRole::Parent => {
                    // Parents may only remove User/Guest roles, not other Admins or Parents.
                    if matches!(target_role, UserRole::Admin | UserRole::Parent) {
                        return Err(
                            "Permission denied: Parents may only remove User or Guest members"
                                .to_string(),
                        );
                    }
                }
                _ => {
                    return Err(
                        "Permission denied: only Admin or Parent roles can remove members"
                            .to_string(),
                    );
                }
            }

            family.users.remove(user_id);
            info!(
                "[FAMILY] Removed user {} from family {}",
                user_id, family_id
            );
            return Ok(());
        }

        Err("Family not found".to_string())
    }

    /// Update user role.
    ///
    /// Only Admin can change role assignments. Additional constraints:
    /// - The family Admin's own role cannot be changed.
    /// - The Admin role cannot be assigned to any user via this method
    ///   (ownership transfer requires a dedicated operation).
    pub async fn update_user_role(
        &self,
        family_id: &str,
        caller_id: &str,
        user_id: &str,
        new_role: UserRole,
    ) -> Result<(), String> {
        let mut families = self.families.write().await;

        if let Some(family) = families.get_mut(family_id) {
            // Only Admin may change roles.
            let caller_role = family
                .users
                .get(caller_id)
                .map(|u| u.role.clone())
                .ok_or_else(|| "Caller is not a member of this family".to_string())?;

            if caller_role != UserRole::Admin {
                return Err(
                    "Permission denied: only Admin can change role assignments".to_string(),
                );
            }

            // The Admin's own role is immutable via this operation.
            if family.admin_id == user_id {
                return Err("Cannot change the family Admin's role".to_string());
            }

            // Assigning the Admin role is not permitted via this path.
            if new_role == UserRole::Admin {
                return Err(
                    "Cannot assign Admin role via update_user_role; \
                     use a dedicated ownership-transfer operation"
                        .to_string(),
                );
            }

            if let Some(user) = family.users.get_mut(user_id) {
                user.role = new_role;
                info!(
                    "[FAMILY] Updated user {} role in family {}",
                    user_id, family_id
                );
                return Ok(());
            }

            return Err("User not found in family".to_string());
        }

        Err("Family not found".to_string())
    }

    /// Create shared memory pool
    pub async fn create_memory_pool(
        &self,
        family_id: &str,
        access_level: MemoryAccessLevel,
    ) -> Result<String, String> {
        let pool_id = uuid::Uuid::new_v4().to_string();

        let sharing = MemorySharing {
            pool_id: pool_id.clone(),
            allowed_users: HashSet::new(), // Empty = all users
            access_level,
            created_at: Utc::now(),
        };

        let mut memory_sharing = self.memory_sharing.write().await;
        memory_sharing.insert(pool_id.clone(), sharing);

        // Add pool to family
        let mut families = self.families.write().await;
        if let Some(family) = families.get_mut(family_id) {
            family.shared_memory_pool_ids.push(pool_id.clone());
            info!("[FAMILY] Created memory pool for family: {}", family_id);
            return Ok(pool_id);
        }

        Err("Family not found".to_string())
    }

    /// Get pending invitations for email
    pub async fn get_pending_invitations(&self, email: &str) -> Vec<Invitation> {
        let invitations = self.invitations.read().await;
        invitations
            .iter()
            .filter(|i| i.email == email && !i.accepted && i.expires_at > Utc::now())
            .cloned()
            .collect()
    }

    /// Log activity
    pub async fn log_activity(&self, user_id: String, action: String, details: String) {
        let entry = ActivityLogEntry {
            timestamp: Utc::now(),
            user_id,
            action,
            details,
            ip_address: None,
        };

        let mut log = self.activity_log.write().await;
        log.push(entry);

        // Keep last 10000 entries
        if log.len() > 10000 {
            log.remove(0);
        }
    }

    /// Get activity log for family
    pub async fn get_family_activity(
        &self,
        family_id: &str,
        limit: usize,
    ) -> Result<Vec<ActivityLogEntry>, String> {
        let families = self.families.read().await;
        let family = families
            .get(family_id)
            .ok_or_else(|| format!("Family not found: {}", family_id))?;

        let user_ids: HashSet<String> = family.users.keys().cloned().collect();
        let log = self.activity_log.read().await;

        let activity: Vec<ActivityLogEntry> = log
            .iter()
            .filter(|e| user_ids.contains(&e.user_id))
            .rev()
            .take(limit)
            .cloned()
            .collect();

        Ok(activity)
    }

    /// Get activity log for user
    pub async fn get_user_activity(&self, user_id: &str, limit: usize) -> Vec<ActivityLogEntry> {
        let log = self.activity_log.read().await;
        log.iter()
            .filter(|e| e.user_id == user_id)
            .rev()
            .take(limit)
            .cloned()
            .collect()
    }
}

impl Default for FamilyModeManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_family() {
        let manager = FamilyModeManager::new();
        let family_id = manager
            .create_family("user1".to_string(), "My Family".to_string(), None)
            .await
            .unwrap();

        let family = manager.get_family(&family_id).await.unwrap();
        assert_eq!(family.name, "My Family");
        assert_eq!(family.admin_id, "user1");
    }

    #[tokio::test]
    async fn test_send_and_accept_invitation() {
        let manager = FamilyModeManager::new();
        let family_id = manager
            .create_family("admin".to_string(), "Family".to_string(), None)
            .await
            .unwrap();

        let inv_id = manager
            .send_invitation(
                &family_id,
                "admin",
                "user@example.com".to_string(),
                UserRole::User,
            )
            .await
            .unwrap();

        // Retrieve the invitation code so we can pass it to accept_invitation.
        let code = {
            let invitations = manager.invitations.read().await;
            invitations
                .iter()
                .find(|i| i.id == inv_id)
                .unwrap()
                .code
                .clone()
        };

        manager
            .accept_invitation(
                &inv_id,
                &code,
                "newuser".to_string(),
                "New User".to_string(),
                "user@example.com",
            )
            .await
            .unwrap();

        let family = manager.get_family(&family_id).await.unwrap();
        assert_eq!(family.users.len(), 2);
    }

    #[tokio::test]
    async fn test_remove_user() {
        let manager = FamilyModeManager::new();
        let family_id = manager
            .create_family("admin".to_string(), "Family".to_string(), None)
            .await
            .unwrap();

        let inv_id = manager
            .send_invitation(
                &family_id,
                "admin",
                "user@example.com".to_string(),
                UserRole::User,
            )
            .await
            .unwrap();

        let code = {
            let invitations = manager.invitations.read().await;
            invitations
                .iter()
                .find(|i| i.id == inv_id)
                .unwrap()
                .code
                .clone()
        };

        manager
            .accept_invitation(
                &inv_id,
                &code,
                "user1".to_string(),
                "User 1".to_string(),
                "user@example.com",
            )
            .await
            .unwrap();

        manager
            .remove_user(&family_id, "admin", "user1")
            .await
            .unwrap();

        let family = manager.get_family(&family_id).await.unwrap();
        assert_eq!(family.users.len(), 1);
    }
}
