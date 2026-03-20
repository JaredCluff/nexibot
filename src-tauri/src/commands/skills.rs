//! Skills management commands

use tauri::State;
use tracing::info;

use crate::session_overrides::SessionOverrides;
use crate::skills::{builtin_templates, Skill, SkillConfig, SkillExecResult, SkillTemplate};

use super::AppState;

/// List all available skills
#[tauri::command]
pub async fn list_skills(state: State<'_, AppState>) -> Result<Vec<Skill>, String> {
    let skills_manager = state.skills_manager.read().await;
    let skills = skills_manager.list_skills();
    Ok(skills.into_iter().cloned().collect())
}

/// Get a specific skill by ID
#[tauri::command]
pub async fn get_skill(state: State<'_, AppState>, skill_id: String) -> Result<Skill, String> {
    let skills_manager = state.skills_manager.read().await;
    skills_manager
        .get_skill(&skill_id)
        .cloned()
        .ok_or_else(|| format!("Skill not found: {}", skill_id))
}

/// Get user-invocable skills (for /commands)
#[tauri::command]
pub async fn list_user_invocable_skills(state: State<'_, AppState>) -> Result<Vec<Skill>, String> {
    let skills_manager = state.skills_manager.read().await;
    let skills = skills_manager.get_user_invocable_skills();
    Ok(skills.into_iter().cloned().collect())
}

/// Reload all skills from disk
#[tauri::command]
pub async fn reload_skills(state: State<'_, AppState>) -> Result<(), String> {
    info!("Reloading skills from disk");
    let mut skills_manager = state.skills_manager.write().await;
    skills_manager.load_all_skills().map_err(|e| e.to_string())
}

/// Create a new skill
#[tauri::command]
pub async fn create_skill(
    state: State<'_, AppState>,
    id: String,
    name: String,
    description: String,
    content: String,
    user_invocable: bool,
) -> Result<Skill, String> {
    let mut skills_manager = state.skills_manager.write().await;
    skills_manager
        .create_skill(&id, &name, &description, &content, user_invocable)
        .map_err(|e| e.to_string())
}

/// Update an existing skill
#[tauri::command]
pub async fn update_skill(
    state: State<'_, AppState>,
    id: String,
    name: Option<String>,
    description: Option<String>,
    content: Option<String>,
    user_invocable: Option<bool>,
) -> Result<Skill, String> {
    let mut skills_manager = state.skills_manager.write().await;
    skills_manager
        .update_skill(
            &id,
            name.as_deref(),
            description.as_deref(),
            content.as_deref(),
            user_invocable,
        )
        .map_err(|e| e.to_string())
}

/// Delete a skill
#[tauri::command]
pub async fn delete_skill(state: State<'_, AppState>, skill_id: String) -> Result<(), String> {
    let mut skills_manager = state.skills_manager.write().await;
    skills_manager
        .delete_skill(&skill_id)
        .map_err(|e| e.to_string())
}

/// List available skill templates
#[tauri::command]
pub async fn list_skill_templates() -> Result<Vec<SkillTemplate>, String> {
    Ok(builtin_templates())
}

/// Reset bundled skills to their default versions.
#[tauri::command]
pub async fn reset_bundled_skills(state: State<'_, AppState>) -> Result<usize, String> {
    let mut skills_manager = state.skills_manager.write().await;
    skills_manager
        .reset_bundled_skills()
        .map_err(|e| e.to_string())
}

/// Execute a script from a skill's `scripts/` directory.
///
/// The script runs under `/bin/sh` with a sanitized environment.
/// Only environment variables explicitly declared in the skill's `env` metadata
/// are forwarded. Per-skill config values from `skill.config.yaml` are injected
/// as `SKILL_CONFIG_<KEY>` variables.
///
/// # Arguments
/// - `skill_id` – Skill identifier.
/// - `script_name` – Filename within `scripts/` (e.g. `"run.sh"`).
/// - `input` – Optional text sent to the script on stdin.
#[tauri::command]
pub async fn invoke_skill(
    state: State<'_, AppState>,
    skill_id: String,
    script_name: String,
    input: Option<String>,
) -> Result<SkillExecResult, String> {
    info!(
        "[SKILLS] Invoking script '{}' for skill '{}'",
        script_name, skill_id
    );
    let skills_manager = state.skills_manager.read().await;
    skills_manager
        .execute_skill_script(&skill_id, &script_name, input.as_deref())
        .await
        .map_err(|e| e.to_string())
}

/// Get the runtime configuration for a skill (from `skill.config.yaml`).
#[tauri::command]
pub async fn get_skill_config(
    state: State<'_, AppState>,
    skill_id: String,
) -> Result<SkillConfig, String> {
    let skills_manager = state.skills_manager.read().await;
    Ok(skills_manager.load_skill_config(&skill_id))
}

/// Save runtime configuration for a skill to `skill.config.yaml`.
///
/// Creates or overwrites `skill.config.yaml` in the skill's directory.
/// Config value keys must match `[A-Z0-9_]+` (validated on write).
#[tauri::command]
pub async fn save_skill_config(
    state: State<'_, AppState>,
    skill_id: String,
    config: SkillConfig,
) -> Result<(), String> {
    let skills_manager = state.skills_manager.read().await;
    let skill = skills_manager
        .get_skill(&skill_id)
        .ok_or_else(|| format!("Skill '{}' not found", skill_id))?;

    // Validate config value keys: only [A-Z0-9_] allowed
    // Validate config values: no null bytes, max 4096 chars (values are injected as env vars)
    for key in config.values.keys() {
        if !key
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        {
            return Err(format!(
                "Invalid config key '{}': only uppercase letters, digits, and underscores are allowed",
                key
            ));
        }
    }
    for (key, value) in &config.values {
        if value.contains('\0') {
            return Err(format!(
                "Invalid config value for key '{}': null bytes are not allowed",
                key
            ));
        }
        if value.len() > 4096 {
            return Err(format!(
                "Invalid config value for key '{}': exceeds maximum length of 4096 characters",
                key
            ));
        }
    }

    let config_path = skill.path.join("skill.config.yaml");
    let yaml = serde_yml::to_string(&config).map_err(|e| e.to_string())?;
    std::fs::write(&config_path, yaml).map_err(|e| e.to_string())?;

    info!("[SKILLS] Saved skill.config.yaml for '{}'", skill_id);
    Ok(())
}

/// Test a skill by sending its instructions to Claude with a test prompt
#[tauri::command]
pub async fn test_skill(state: State<'_, AppState>, skill_id: String) -> Result<String, String> {
    let skills_manager = state.skills_manager.read().await;
    let skill = skills_manager
        .get_skill(&skill_id)
        .ok_or_else(|| format!("Skill not found: {}", skill_id))?;

    let test_prompt = format!(
        "You have the following skill:\n\n{}\n\nDemonstrate this skill briefly with a short example.",
        skill.content
    );

    drop(skills_manager);

    let claude = state.claude_client.read().await;
    let default_overrides = SessionOverrides::default();

    match claude
        .send_message_with_tools(&test_prompt, &[], &default_overrides)
        .await
    {
        Ok(result) => Ok(result.text),
        Err(e) => Err(format!("Test failed: {}", e)),
    }
}
