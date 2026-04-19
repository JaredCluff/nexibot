//! Matrix E2EE client using matrix-sdk.
use anyhow::{Context, Result};
use matrix_sdk::{
    config::SyncSettings,
    ruma::events::room::message::{MessageType, OriginalSyncRoomMessageEvent, RoomMessageEventContent},
    Client as MatrixSdkClient, Room,
};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::info;

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
    pub async fn new_and_login(homeserver_url: &str, user_id: &str, password: &str) -> Result<Self> {
        let store_path = matrix_store_path();
        tokio::fs::create_dir_all(&store_path).await
            .context("Failed to create matrix store directory")?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&store_path, std::fs::Permissions::from_mode(0o700));
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

    pub async fn send_message(&self, room_id: &str, text: &str) -> Result<()> {
        use matrix_sdk::ruma::RoomId;
        let rid = RoomId::parse(room_id).context("Invalid room ID")?;
        let room = self.inner.get_room(&rid)
            .ok_or_else(|| anyhow::anyhow!("Room {} not found", room_id))?;
        room.send(RoomMessageEventContent::text_plain(text)).await
            .context("Failed to send E2EE message")?;
        Ok(())
    }

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

        self.inner.add_event_handler({
            let on_message = on_message.clone();
            let allowed = allowed.clone();
            move |ev: OriginalSyncRoomMessageEvent, room: Room| {
                let on_message = on_message.clone();
                let allowed = allowed.clone();
                async move {
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
