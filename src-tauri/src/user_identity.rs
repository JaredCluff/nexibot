//! Cross-channel user identity layer.
//!
//! Allows the same human across Telegram, GUI, WhatsApp, and Voice
//! to be recognized as one user with shared conversation context.
//!
//! Users are auto-created on first contact from a channel. Multiple channels
//! can be linked to one user identity via pairing codes or manual linking.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::channel::ChannelSource;

/// A user identity that can span multiple channels.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserIdentity {
    /// Unique user ID (UUID).
    pub id: String,
    /// Human-readable display name.
    pub display_name: String,
    /// All channel bindings for this user.
    pub channel_bindings: Vec<ChannelIdentity>,
    /// When this user was first seen.
    pub created_at: DateTime<Utc>,
    /// Last activity timestamp.
    pub last_active: DateTime<Utc>,
}

/// A single channel binding for a user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelIdentity {
    /// Channel type: "telegram", "whatsapp", "gui", "voice".
    pub channel_type: String,
    /// Channel-specific peer ID (e.g., Telegram chat_id, WhatsApp phone, "local").
    pub peer_id: String,
    /// When this channel was linked.
    pub linked_at: DateTime<Utc>,
}

/// Serializable user info for the frontend.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    pub id: String,
    pub display_name: String,
    pub channel_bindings: Vec<ChannelIdentity>,
    pub created_at: DateTime<Utc>,
    pub last_active: DateTime<Utc>,
}

impl From<&UserIdentity> for UserInfo {
    fn from(u: &UserIdentity) -> Self {
        Self {
            id: u.id.clone(),
            display_name: u.display_name.clone(),
            channel_bindings: u.channel_bindings.clone(),
            created_at: u.created_at,
            last_active: u.last_active,
        }
    }
}

/// Persisted user identity data.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct UserIdentityStore {
    users: Vec<UserIdentity>,
}

/// Manages user identities across channels.
pub struct UserIdentityManager {
    /// All known users, keyed by user ID.
    users: HashMap<String, UserIdentity>,
    /// Fast lookup: (channel_type, peer_id) -> user_id.
    channel_index: HashMap<(String, String), String>,
    /// Where to persist the identity store.
    storage_path: PathBuf,
}

#[allow(dead_code)]
impl UserIdentityManager {
    /// Get the data directory for user identity storage.
    fn get_data_dir() -> Result<PathBuf> {
        let home = dirs::home_dir().context("Failed to get home directory")?;
        Ok(home.join(".config/nexibot/identity"))
    }

    /// Create a new manager, loading persisted data if available.
    pub fn new() -> Result<Self> {
        let data_dir = Self::get_data_dir()?;
        std::fs::create_dir_all(&data_dir)?;
        let storage_path = data_dir.join("user_identities.json");
        let mut manager = Self {
            users: HashMap::new(),
            channel_index: HashMap::new(),
            storage_path,
        };

        if let Err(e) = manager.load() {
            warn!("[USER-IDENTITY] Failed to load persisted identities: {}", e);
        }

        // Always ensure a "local" GUI user exists
        if manager
            .channel_index
            .get(&("gui".to_string(), "local".to_string()))
            .is_none()
        {
            let user = UserIdentity {
                id: uuid::Uuid::new_v4().to_string(),
                display_name: "Local User".to_string(),
                channel_bindings: vec![ChannelIdentity {
                    channel_type: "gui".to_string(),
                    peer_id: "local".to_string(),
                    linked_at: Utc::now(),
                }],
                created_at: Utc::now(),
                last_active: Utc::now(),
            };
            let id = user.id.clone();
            manager
                .channel_index
                .insert(("gui".to_string(), "local".to_string()), id.clone());
            manager.users.insert(id, user);
            if let Err(e) = manager.save() {
                warn!(
                    "[USER-IDENTITY] Failed to persist default GUI user during startup: {}",
                    e
                );
            }
        }

        Ok(manager)
    }

    /// Resolve or auto-create a user for a given channel/peer.
    ///
    /// If the (channel, peer_id) is already known, returns the existing user.
    /// Otherwise, creates a new user with this channel binding.
    pub fn resolve_user(&mut self, channel: &ChannelSource) -> Result<&UserIdentity> {
        let (channel_type, peer_id) = channel_key(channel);
        let key = (channel_type.clone(), peer_id.clone());

        if let Some(user_id) = self.channel_index.get(&key) {
            let user_id = user_id.clone();
            let user = self
                .users
                .get_mut(&user_id)
                .ok_or_else(|| anyhow::anyhow!("channel_index and users out of sync for user '{}'", user_id))?;
            user.last_active = Utc::now();
            return self
                .users
                .get(&user_id)
                .ok_or_else(|| anyhow::anyhow!("users map missing key '{}' after get_mut succeeded", user_id));
        }

        // Auto-create
        let now = Utc::now();
        let user_id = uuid::Uuid::new_v4().to_string();
        let display_name = format!("{}/{}", channel_type, peer_id);

        let user = UserIdentity {
            id: user_id.clone(),
            display_name,
            channel_bindings: vec![ChannelIdentity {
                channel_type: channel_type.clone(),
                peer_id: peer_id.clone(),
                linked_at: now,
            }],
            created_at: now,
            last_active: now,
        };

        info!(
            "[USER-IDENTITY] Auto-created user '{}' for {}/{}",
            user_id, channel_type, peer_id
        );

        self.channel_index.insert(key, user_id.clone());
        self.users.insert(user_id.clone(), user);
        if let Err(e) = self.save() {
            warn!(
                "[USER-IDENTITY] Failed to persist auto-created user '{}' for {}/{}: {}",
                user_id, channel_type, peer_id, e
            );
        }

        self.users
            .get(&user_id)
            .ok_or_else(|| anyhow::anyhow!("users map missing key '{}' after insert", user_id))
    }

    /// Link a new channel binding to an existing user.
    pub fn link_channel(&mut self, user_id: &str, channel_type: &str, peer_id: &str) -> Result<()> {
        let previous_users = self.users.clone();
        let previous_channel_index = self.channel_index.clone();
        let key = (channel_type.to_string(), peer_id.to_string());

        // Check if this channel is already linked to another user
        if let Some(existing_uid) = self.channel_index.get(&key) {
            if existing_uid == user_id {
                return Ok(()); // Already linked to this user
            }
            anyhow::bail!(
                "Channel {}/{} is already linked to user '{}'",
                channel_type,
                peer_id,
                existing_uid
            );
        }

        let user = self
            .users
            .get_mut(user_id)
            .context(format!("User '{}' not found", user_id))?;

        user.channel_bindings.push(ChannelIdentity {
            channel_type: channel_type.to_string(),
            peer_id: peer_id.to_string(),
            linked_at: Utc::now(),
        });

        self.channel_index.insert(key, user_id.to_string());
        info!(
            "[USER-IDENTITY] Linked {}/{} to user '{}'",
            channel_type, peer_id, user_id
        );
        if let Err(e) = self.save() {
            self.users = previous_users;
            self.channel_index = previous_channel_index;
            return Err(e);
        }
        Ok(())
    }

    /// Unlink a channel binding from a user.
    pub fn unlink_channel(
        &mut self,
        user_id: &str,
        channel_type: &str,
        peer_id: &str,
    ) -> Result<()> {
        let previous_users = self.users.clone();
        let previous_channel_index = self.channel_index.clone();
        let key = (channel_type.to_string(), peer_id.to_string());

        let user = self
            .users
            .get_mut(user_id)
            .context(format!("User '{}' not found", user_id))?;

        let before = user.channel_bindings.len();
        user.channel_bindings
            .retain(|b| !(b.channel_type == channel_type && b.peer_id == peer_id));

        if user.channel_bindings.len() == before {
            anyhow::bail!(
                "Channel {}/{} not linked to user '{}'",
                channel_type,
                peer_id,
                user_id
            );
        }

        self.channel_index.remove(&key);
        info!(
            "[USER-IDENTITY] Unlinked {}/{} from user '{}'",
            channel_type, peer_id, user_id
        );
        if let Err(e) = self.save() {
            self.users = previous_users;
            self.channel_index = previous_channel_index;
            return Err(e);
        }
        Ok(())
    }

    /// Get a user by ID.
    pub fn get_user(&self, user_id: &str) -> Option<&UserIdentity> {
        self.users.get(user_id)
    }

    /// Look up a user by channel binding.
    pub fn get_user_by_channel(&self, channel_type: &str, peer_id: &str) -> Option<&UserIdentity> {
        let key = (channel_type.to_string(), peer_id.to_string());
        self.channel_index
            .get(&key)
            .and_then(|uid| self.users.get(uid))
    }

    /// List all users.
    pub fn list_users(&self) -> Vec<UserInfo> {
        self.users.values().map(UserInfo::from).collect()
    }

    /// Update a user's display name.
    pub fn set_display_name(&mut self, user_id: &str, name: &str) -> Result<()> {
        let previous_users = self.users.clone();
        let previous_channel_index = self.channel_index.clone();
        let user = self
            .users
            .get_mut(user_id)
            .context(format!("User '{}' not found", user_id))?;
        user.display_name = name.to_string();
        if let Err(e) = self.save() {
            self.users = previous_users;
            self.channel_index = previous_channel_index;
            return Err(e);
        }
        Ok(())
    }

    /// Delete a user and all their channel bindings.
    pub fn delete_user(&mut self, user_id: &str) -> Result<()> {
        let previous_users = self.users.clone();
        let previous_channel_index = self.channel_index.clone();
        let user = self
            .users
            .remove(user_id)
            .context(format!("User '{}' not found", user_id))?;

        for binding in &user.channel_bindings {
            let key = (binding.channel_type.clone(), binding.peer_id.clone());
            self.channel_index.remove(&key);
        }

        info!(
            "[USER-IDENTITY] Deleted user '{}' ({} bindings removed)",
            user_id,
            user.channel_bindings.len()
        );
        if let Err(e) = self.save() {
            self.users = previous_users;
            self.channel_index = previous_channel_index;
            return Err(e);
        }
        Ok(())
    }

    /// Persist to disk.
    fn save(&self) -> Result<()> {
        let store = UserIdentityStore {
            users: self.users.values().cloned().collect(),
        };
        let json = serde_json::to_string_pretty(&store)?;

        if let Some(parent) = self.storage_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(&self.storage_path, json).context("Failed to write user identities")?;
        Ok(())
    }

    /// Load from disk.
    fn load(&mut self) -> Result<()> {
        if !self.storage_path.exists() {
            return Ok(());
        }

        let json = std::fs::read_to_string(&self.storage_path)
            .context("Failed to read user identities")?;
        let store: UserIdentityStore =
            serde_json::from_str(&json).context("Failed to parse user identities")?;

        for user in store.users {
            for binding in &user.channel_bindings {
                let key = (binding.channel_type.clone(), binding.peer_id.clone());
                self.channel_index.insert(key, user.id.clone());
            }
            self.users.insert(user.id.clone(), user);
        }

        info!(
            "[USER-IDENTITY] Loaded {} users from disk",
            self.users.len()
        );
        Ok(())
    }
}

/// Extract (channel_type, peer_id) from a ChannelSource.
#[allow(dead_code)]
fn channel_key(source: &ChannelSource) -> (String, String) {
    match source {
        ChannelSource::Gui => ("gui".to_string(), "local".to_string()),
        ChannelSource::Telegram { chat_id } => ("telegram".to_string(), chat_id.to_string()),
        ChannelSource::WhatsApp { phone_number } => ("whatsapp".to_string(), phone_number.clone()),
        ChannelSource::Voice => ("voice".to_string(), "local".to_string()),
        ChannelSource::InterAgent { agent_id } => ("interagent".to_string(), agent_id.clone()),
        ChannelSource::Discord { channel_id, .. } => {
            ("discord".to_string(), channel_id.to_string())
        }
        ChannelSource::Slack { channel_id } => ("slack".to_string(), channel_id.clone()),
        ChannelSource::Signal { phone_number } => ("signal".to_string(), phone_number.clone()),
        ChannelSource::Teams { conversation_id } => ("teams".to_string(), conversation_id.clone()),
        ChannelSource::Matrix { room_id } => ("matrix".to_string(), room_id.clone()),
        ChannelSource::BlueBubbles { chat_guid } => ("bluebubbles".to_string(), chat_guid.clone()),
        ChannelSource::GoogleChat {
            space_id,
            sender_id,
        } => (
            "google_chat".to_string(),
            format!("{}:{}", space_id, sender_id),
        ),
        ChannelSource::Mattermost { channel_id } => ("mattermost".to_string(), channel_id.clone()),
        ChannelSource::Messenger { sender_id } => ("messenger".to_string(), sender_id.clone()),
        ChannelSource::Instagram { sender_id } => ("instagram".to_string(), sender_id.clone()),
        ChannelSource::Line { user_id, .. } => ("line".to_string(), user_id.clone()),
        ChannelSource::Twilio { phone_number } => ("twilio".to_string(), phone_number.clone()),
        ChannelSource::Mastodon { account_id } => ("mastodon".to_string(), account_id.clone()),
        ChannelSource::RocketChat { room_id } => ("rocketchat".to_string(), room_id.clone()),
        ChannelSource::WebChat { session_id } => ("webchat".to_string(), session_id.clone()),
        ChannelSource::Email { thread_id } => ("email".to_string(), thread_id.clone()),
        ChannelSource::Gmail { thread_id } => ("gmail".to_string(), thread_id.clone()),
        ChannelSource::Nats { sender } => ("nats".to_string(), sender.clone()),
    }
}
