//! ClawHub marketplace Tauri commands.

use serde::{Deserialize, Serialize};
use tauri::State;
use tracing::{error, info};

use crate::clawhub::{ClawHubClient, ClawHubSkillSummary, InstallResult};
use crate::skill_security::{self, SkillSecurityReport};

use super::AppState;

/// Search the ClawHub marketplace.
#[tauri::command]
pub async fn search_clawhub(
    query: String,
    limit: Option<u32>,
) -> Result<Vec<ClawHubSkillSummary>, String> {
    info!("[CLAWHUB_CMD] Searching for: {}", query);
    let client = ClawHubClient::new();
    client
        .search(&query, limit.unwrap_or(20))
        .await
        .map_err(|e| {
            error!("[CLAWHUB_CMD] Search failed: {}", e);
            e.to_string()
        })
}

/// Get detailed skill info from ClawHub, including a pre-computed security report.
#[derive(Debug, Serialize, Deserialize)]
pub struct ClawHubSkillInfoResponse {
    pub slug: String,
    pub name: String,
    pub description: String,
    pub author: String,
    pub version: String,
    pub downloads: u64,
    pub rating: f32,
    pub tags: Vec<String>,
    pub security_report: SkillSecurityReport,
}

#[tauri::command]
pub async fn get_clawhub_skill_info(slug: String) -> Result<ClawHubSkillInfoResponse, String> {
    info!("[CLAWHUB_CMD] Getting skill info: {}", slug);
    let client = ClawHubClient::new();

    let detail = client.get_skill(&slug).await.map_err(|e| {
        error!("[CLAWHUB_CMD] Get skill failed: {}", e);
        e.to_string()
    })?;

    // Run security analysis on the fetched content
    let scripts_content: String = detail
        .scripts
        .values()
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    let full_content = format!("{}\n\n{}", detail.skill_md, scripts_content);

    let temp_metadata = crate::skills::SkillMetadata {
        name: Some(detail.name.clone()),
        description: Some(detail.description.clone()),
        user_invocable: true,
        disable_model_invocation: false,
        requirements: Vec::new(),
        metadata: detail.metadata.clone(),
        command_dispatch: None,
        command_tool: None,
        command_arg_mode: None,
        version: Some(detail.version.clone()),
        author: Some(detail.author.clone()),
        source: Some("clawhub".to_string()),
    };

    let security_report = skill_security::analyze_skill_content(
        &detail.name,
        &full_content,
        None,
        Some(&temp_metadata),
    );

    Ok(ClawHubSkillInfoResponse {
        slug: detail.slug,
        name: detail.name,
        description: detail.description,
        author: detail.author,
        version: detail.version,
        downloads: detail.downloads,
        rating: detail.rating,
        tags: detail.tags,
        security_report,
    })
}

/// Install a skill from ClawHub with security gate.
#[tauri::command]
pub async fn install_clawhub_skill(
    slug: String,
    force_install: Option<bool>,
    state: State<'_, AppState>,
) -> Result<InstallResult, String> {
    info!(
        "[CLAWHUB_CMD] Installing skill: {} (force: {:?})",
        slug, force_install
    );

    let client = ClawHubClient::new();
    let mut skills_manager = state.skills_manager.write().await;

    client
        .install_skill(&slug, &mut skills_manager, force_install.unwrap_or(false))
        .await
        .map_err(|e| {
            error!("[CLAWHUB_CMD] Install failed: {}", e);
            e.to_string()
        })
}

/// Analyze security of an already-installed skill.
#[tauri::command]
pub async fn analyze_skill_security(
    skill_id: String,
    state: State<'_, AppState>,
) -> Result<SkillSecurityReport, String> {
    info!("[CLAWHUB_CMD] Analyzing skill security: {}", skill_id);

    let skills_manager = state.skills_manager.read().await;
    let skill = skills_manager
        .get_skill(&skill_id)
        .ok_or_else(|| format!("Skill '{}' not found", skill_id))?;

    Ok(skill_security::analyze_skill(skill))
}
