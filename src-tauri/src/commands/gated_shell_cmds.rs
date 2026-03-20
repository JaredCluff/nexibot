//! Tauri commands for NexiGate gated shell control.

use tauri::{command, AppHandle, Manager, State};

use crate::commands::AppState;
use crate::gated_shell::audit::AuditEntry;
use crate::gated_shell::session::ShellSessionInfo;
use crate::gated_shell::tmux_bridge::{TmuxSessionInfo, TmuxWaitResult};
use crate::gated_shell::{GatedShell, GatedShellStatus};

/// Return type for `sign_plugin_file`.
#[derive(Debug, serde::Serialize)]
pub struct SignedPluginResult {
    pub manifest_path: String,
    pub content_sha256: String,
    pub public_key_hex: String,
}

fn shell(state: &AppState) -> Result<&GatedShell, String> {
    state
        .gated_shell
        .as_deref()
        .ok_or_else(|| "Gated shell is not available in headless mode".to_string())
}

/// Enable or disable the gated shell and return the new status.
#[command]
pub async fn set_gated_shell_enabled(
    enabled: bool,
    state: State<'_, AppState>,
) -> Result<GatedShellStatus, String> {
    let gs = shell(&state)?;
    gs.set_enabled(enabled).await;
    Ok(gs.status().await)
}

/// Enable or disable debug mode (full raw PTY output in audit log).
#[command]
pub async fn set_gated_shell_debug(
    debug_mode: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    shell(&state)?.set_debug_mode(debug_mode).await;
    Ok(())
}

/// Enable or disable session recording to asciicast files.
#[command]
pub async fn set_gated_shell_record(
    enabled: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    shell(&state)?.set_record_sessions(enabled).await;
    Ok(())
}

/// Get current gated shell status.
#[command]
pub async fn get_gated_shell_status(
    state: State<'_, AppState>,
) -> Result<GatedShellStatus, String> {
    Ok(shell(&state)?.status().await)
}

/// List all active shell sessions.
#[command]
pub async fn list_shell_sessions(
    state: State<'_, AppState>,
) -> Result<Vec<ShellSessionInfo>, String> {
    Ok(shell(&state)?.list_sessions().await)
}

/// Get audit log entries for a session (most recent first).
#[command]
pub async fn get_shell_audit_log(
    session_key: String,
    limit: usize,
    state: State<'_, AppState>,
) -> Result<Vec<AuditEntry>, String> {
    let limit = limit.min(1000);
    Ok(shell(&state)?.get_audit_log(&session_key, limit).await)
}

/// Close a shell session.
#[command]
pub async fn close_shell_session(
    session_key: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    shell(&state)?.close_session(&session_key).await;
    Ok(())
}

/// Generate a new Ed25519 signing keypair for plugin signing.
///
/// Returns the hex-encoded public key (32 bytes = 64 hex chars) for use in
/// `config.gated_shell.plugins.trusted_keys`. The private key (hex seed, 32 bytes)
/// is returned once and should be stored securely at
/// the NexiBot shell plugins directory as `signing.key` (with restricted permissions).
///
/// This is a UI-only tool — never exposed to the LLM as a tool.
#[command]
pub async fn generate_plugin_signing_key() -> Result<serde_json::Value, String> {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    let mut csprng = OsRng;
    let signing_key = SigningKey::generate(&mut csprng);
    let verifying_key = signing_key.verifying_key();

    let private_hex = hex::encode(signing_key.to_bytes());
    let public_hex = hex::encode(verifying_key.as_bytes());

    Ok(serde_json::json!({
        "private_key_hex": private_hex,
        "public_key_hex": public_hex,
        "note": "Store private_key_hex securely (restrict to owner-only access). Add public_key_hex to config trusted_keys."
    }))
}

/// Sign a Rhai plugin file and write a `.manifest.json` alongside it.
///
/// Parameters:
/// - `plugin_path`: Absolute path to the `.rhai` script file.
/// - `private_key_hex`: Hex-encoded 32-byte Ed25519 signing key seed.
/// - `author`: Author string for the manifest.
/// - `description`: Plugin description.
///
/// Writes `{plugin_path_stem}.manifest.json` next to the script.
/// Returns the manifest path and content hash for verification.
///
/// This is a UI-only tool — never exposed to the LLM as a tool.
#[command]
pub async fn sign_plugin_file(
    plugin_path: String,
    private_key_hex: String,
    author: String,
    description: String,
) -> Result<SignedPluginResult, String> {
    use base64::Engine as _;
    use ed25519_dalek::{Signer, SigningKey};
    use sha2::{Digest, Sha256};
    use std::path::Path;
    use zeroize::Zeroizing;

    // Validate metadata field lengths to prevent oversized manifest files
    if author.len() > 256 {
        return Err("author must be 256 characters or fewer".to_string());
    }
    if description.len() > 1024 {
        return Err("description must be 1024 characters or fewer".to_string());
    }

    // Decode the private key and hold bytes in a zeroize-on-drop wrapper so they
    // are cleared from memory when this scope exits (protects against crash dumps).
    let key_bytes = Zeroizing::new(
        hex::decode(&private_key_hex)
            .map_err(|_| "private_key_hex is not valid hex".to_string())?,
    );
    if key_bytes.len() != 32 {
        return Err("private_key_hex must be 64 hex chars (32 bytes)".to_string());
    }
    let mut key_arr = Zeroizing::new([0u8; 32]);
    key_arr.copy_from_slice(&key_bytes);
    let signing_key = SigningKey::from_bytes(&*key_arr);
    let verifying_key = signing_key.verifying_key();

    // Canonicalize and validate the path before any I/O.
    // This prevents directory traversal (e.g. "../../etc/passwd") and
    // restricts signing to .rhai files only.
    let canonical = tokio::fs::canonicalize(&plugin_path)
        .await
        .map_err(|e| format!("Cannot resolve plugin path '{}': {}", plugin_path, e))?;
    if canonical.extension().and_then(|e| e.to_str()) != Some("rhai") {
        return Err("Only .rhai files can be signed as plugins".to_string());
    }

    // Workspace confinement: signing is restricted to the skills directory to
    // prevent a compromised webview from triggering reads + manifest writes on
    // arbitrary .rhai files elsewhere on disk.
    let skills_dir = crate::skills::SkillsManager::get_skills_dir()
        .map_err(|e| format!("Cannot determine skills directory: {}", e))?;
    let canonical_skills_dir = tokio::fs::canonicalize(&skills_dir)
        .await
        .map_err(|_| {
            "Skills directory does not exist; cannot sign plugin outside a known skills directory"
                .to_string()
        })?;
    if !canonical.starts_with(&canonical_skills_dir) {
        return Err(format!(
            "Plugin path must be within the skills directory ({})",
            canonical_skills_dir.display()
        ));
    }

    // Read the script
    let script_bytes = tokio::fs::read(&canonical)
        .await
        .map_err(|e| format!("Cannot read plugin file '{}': {}", canonical.display(), e))?;

    // Compute SHA-256
    let hash: [u8; 32] = Sha256::digest(&script_bytes).into();
    let content_sha256 = hex::encode(hash);

    // Sign
    let sig = signing_key.sign(&hash);
    let signature_b64 = base64::engine::general_purpose::STANDARD.encode(sig.to_bytes());
    let public_key_b64 = base64::engine::general_purpose::STANDARD.encode(verifying_key.as_bytes());
    let public_key_hex = hex::encode(verifying_key.as_bytes());

    // Derive plugin name from file stem (using canonical path)
    let path = canonical.as_path();
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    // Build manifest
    let manifest = crate::gated_shell::plugin::PluginManifest {
        name: name.clone(),
        version: "1.0.0".to_string(),
        author,
        description,
        plugin_type: crate::gated_shell::plugin::PluginType::Rhai,
        content_sha256: content_sha256.clone(),
        signature: signature_b64,
        public_key: public_key_b64,
    };

    // Write manifest next to the script
    let manifest_path = path.with_file_name(format!("{}.manifest.json", name));
    let manifest_json = serde_json::to_string_pretty(&manifest)
        .map_err(|e| format!("Manifest serialization failed: {}", e))?;

    tokio::fs::write(&manifest_path, manifest_json.as_bytes())
        .await
        .map_err(|e| {
            format!(
                "Cannot write manifest to '{}': {}",
                manifest_path.display(),
                e
            )
        })?;

    Ok(SignedPluginResult {
        manifest_path: manifest_path.to_string_lossy().to_string(),
        content_sha256,
        public_key_hex,
    })
}

// ===========================================================================
// Tmux interactive agent commands (UI-accessible; also called from LLM tool)
// ===========================================================================

/// Start a new tmux interactive agent session.
#[command]
pub async fn start_interactive_session(
    agent_type: String,
    program: String,
    args: Vec<String>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    use crate::security::safe_bins;
    // Validate that the IPC-supplied program resolves to a trusted system binary.
    // Prevents WebView-originating IPC calls from spawning arbitrary executables
    // (same guard applied to the LLM tool path in interactive_tool.rs).
    let validated = safe_bins::validate_binary(&program)
        .map_err(|e| format!("Program '{}' rejected: {}", program, e))?;
    let validated_str = validated.to_string_lossy();

    shell(&state)?
        .tmux_bridge
        .start_session(&agent_type, &validated_str, &args)
        .await
        .map_err(|e| e.to_string())
}

/// Send keystrokes to an interactive session.
#[command]
pub async fn send_to_interactive_session(
    session_id: String,
    input: String,
    send_enter: Option<bool>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    shell(&state)?
        .tmux_bridge
        .send_keys(&session_id, &input, send_enter.unwrap_or(true))
        .await
        .map_err(|e| e.to_string())
}

/// Capture the current pane content of an interactive session.
#[command]
pub async fn read_interactive_session(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    shell(&state)?
        .tmux_bridge
        .capture_pane(&session_id)
        .await
        .map_err(|e| e.to_string())
}

/// Wait for an interactive session to reach one of the specified states.
#[command]
pub async fn wait_for_interactive_session(
    session_id: String,
    wait_for: Vec<String>,
    timeout_ms: Option<u64>,
    state: State<'_, AppState>,
) -> Result<TmuxWaitResult, String> {
    use crate::gated_shell::tmux_bridge::TmuxState;

    fn parse_state(s: &str) -> Option<TmuxState> {
        match s {
            "Starting" => Some(TmuxState::Starting),
            "Ready" => Some(TmuxState::Ready),
            "Running" => Some(TmuxState::Running),
            "Approval" => Some(TmuxState::Approval),
            "Error" => Some(TmuxState::Error),
            "UnknownStable" => Some(TmuxState::UnknownStable),
            "Stopped" => Some(TmuxState::Stopped),
            "Timeout" => Some(TmuxState::Timeout),
            _ => None,
        }
    }

    let target_states: Vec<TmuxState> = wait_for.iter().filter_map(|s| parse_state(s)).collect();

    shell(&state)?
        .tmux_bridge
        .wait_for_state(&session_id, &target_states, timeout_ms)
        .await
        .map_err(|e| e.to_string())
}

/// Stop (kill) an interactive session.
#[command]
pub async fn stop_interactive_session(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    shell(&state)?
        .tmux_bridge
        .stop_session(&session_id)
        .await
        .map_err(|e| e.to_string())
}

/// List all active interactive sessions.
#[command]
pub async fn list_interactive_sessions(
    state: State<'_, AppState>,
) -> Result<Vec<TmuxSessionInfo>, String> {
    Ok(shell(&state)?.tmux_bridge.list_sessions().await)
}

// ===========================================================================
// Shell Viewer window
// ===========================================================================

/// Open the NexiGate Shell Viewer window.
#[command]
pub async fn open_shell_viewer(app: AppHandle) -> Result<(), String> {
    use tauri::webview::WebviewWindowBuilder;
    use tauri::WebviewUrl;

    // If viewer already exists, just focus it
    if let Some(window) = app.get_webview_window("shell-viewer") {
        let _ = window.show();
        let _ = window.set_focus();
        return Ok(());
    }

    // Create new viewer window
    WebviewWindowBuilder::new(&app, "shell-viewer", WebviewUrl::App("index.html".into()))
        .title("NexiGate Shell Viewer")
        .inner_size(1200.0, 800.0)
        .min_inner_size(800.0, 500.0)
        .build()
        .map_err(|e| format!("Failed to open shell viewer: {}", e))?;

    Ok(())
}
