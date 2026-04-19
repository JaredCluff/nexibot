//! Matrix E2EE client using matrix-sdk.
use anyhow::{Context, Result};
use matrix_sdk::{
    config::SyncSettings,
    ruma::events::room::message::{MessageType, OriginalSyncRoomMessageEvent, RoomMessageEventContent},
    Client as MatrixSdkClient, Room,
};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, warn};

pub fn matrix_store_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("nexibot")
        .join("matrix")
}

pub struct MatrixE2EEClient {
    inner: MatrixSdkClient,
    user_id: String,
}

impl MatrixE2EEClient {
    /// Initialize the client and log in using `password`.
    ///
    /// On first run the password is used to authenticate with the homeserver. The SDK persists
    /// the session to `matrix_store_path()` so subsequent calls restore from the SQLite store
    /// without needing the password again.
    ///
    /// Returns `Err` if E2EE cannot be initialized — callers should fall back to `MatrixAdapter`.
    pub async fn new_and_login(homeserver_url: &str, user_id: &str, password: &str) -> Result<Self> {
        // SSRF guard: homeserver_url must not point to internal network addresses.
        {
            use crate::security::ssrf::{self, SsrfPolicy};
            if let Err(e) = ssrf::validate_outbound_request(homeserver_url, &SsrfPolicy::default(), &[]) {
                return Err(anyhow::anyhow!("Matrix E2EE homeserver URL blocked by SSRF policy: {}", e));
            }
        }

        let store_path = matrix_store_path();
        tokio::fs::create_dir_all(&store_path).await
            .context("Failed to create matrix store directory")?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Err(e) = std::fs::set_permissions(&store_path, std::fs::Permissions::from_mode(0o700)) {
                warn!("[MATRIX_E2EE] Failed to restrict store directory permissions (key material at risk): {}", e);
            }
        }

        let client = MatrixSdkClient::builder()
            .homeserver_url(homeserver_url)
            .sqlite_store(&store_path, None)
            .build()
            .await
            .context("Failed to build matrix-sdk client")?;

        let auth = client.matrix_auth();
        if auth.logged_in() {
            info!("[MATRIX_E2EE] Restored existing session for {}", user_id);
        } else {
            auth.login_username(user_id, password)
                .initial_device_display_name("NexiBot")
                .send()
                .await
                .context("Matrix E2EE login failed")?;
            info!("[MATRIX_E2EE] Logged in as {}", user_id);
        }

        Ok(Self { inner: client, user_id: user_id.to_string() })
    }

    /// Send a plain-text message to a room (encrypted if the room supports it).
    pub async fn send_message(&self, room_id: &str, text: &str) -> Result<()> {
        use matrix_sdk::ruma::RoomId;
        let rid = RoomId::parse(room_id).context("Invalid room ID")?;
        let room = self.inner.get_room(&rid)
            .ok_or_else(|| anyhow::anyhow!("Room {} not found", room_id))?;
        room.send(RoomMessageEventContent::text_plain(text)).await
            .context("Failed to send E2EE message")?;
        Ok(())
    }

    /// Start the sync loop. Calls `on_message` for each text message received in allowed rooms.
    ///
    /// Filters out messages sent by the bot itself to prevent feedback loops.
    /// Blocks until the connection is lost or an unrecoverable error occurs.
    pub async fn run_sync_loop<F, Fut>(
        &self,
        allowed_room_ids: Vec<String>,
        on_message: F,
    ) -> Result<()>
    where
        F: Fn(String, String, String) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = ()> + Send,
    {
        let on_message = Arc::new(on_message);
        let allowed = Arc::new(allowed_room_ids);
        let bot_user_id = Arc::new(self.user_id.clone());

        self.inner.add_event_handler({
            let on_message = on_message.clone();
            let allowed = allowed.clone();
            let bot_user_id = bot_user_id.clone();
            move |ev: OriginalSyncRoomMessageEvent, room: Room| {
                let on_message = on_message.clone();
                let allowed = allowed.clone();
                let bot_user_id = bot_user_id.clone();
                async move {
                    // Ignore the bot's own messages to prevent feedback loops.
                    if ev.sender.as_str() == bot_user_id.as_str() {
                        return;
                    }
                    let room_id = room.room_id().to_string();
                    if !allowed.is_empty() && !allowed.contains(&room_id) {
                        return;
                    }
                    let sender = ev.sender.to_string();
                    if let MessageType::Text(text_content) = ev.content.msgtype {
                        on_message(room_id, sender, text_content.body).await;
                    }
                }
            }
        });

        let settings = SyncSettings::default().timeout(std::time::Duration::from_secs(30));
        info!("[MATRIX_E2EE] Starting sync loop for {}", self.user_id);
        self.inner.sync(settings).await.context("Matrix sync loop ended")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_sdk_available() {
        let _ = std::marker::PhantomData::<matrix_sdk::Client>;
    }

    #[test]
    fn matrix_store_path_contains_nexibot_matrix() {
        let p = matrix_store_path();
        let s = p.to_string_lossy();
        assert!(s.contains("nexibot") && s.contains("matrix"), "path: {}", s);
    }

    #[test]
    fn e2ee_client_builder_smoke() {
        let store_path = matrix_store_path();
        assert!(!store_path.to_string_lossy().is_empty());
    }

    #[test]
    fn e2ee_enabled_flag_readable() {
        let cfg = crate::config::channels::MatrixConfig::default();
        assert!(!cfg.e2ee_enabled);
    }
}
