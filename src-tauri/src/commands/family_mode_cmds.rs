//! Tauri commands for Multi-User/Family Mode

use crate::family_mode::{FamilyModeManager, MemoryAccessLevel, UserRole};
use serde_json::json;
use tauri::State;
use tracing::info;

#[tauri::command]
pub async fn create_family(
    family_mode: State<'_, std::sync::Arc<FamilyModeManager>>,
    admin_id: String,
    name: String,
    description: Option<String>,
) -> Result<String, String> {
    info!("[FAMILY] Creating family: {}", name);
    family_mode.create_family(admin_id, name, description).await
}

#[tauri::command]
pub async fn get_family(
    family_mode: State<'_, std::sync::Arc<FamilyModeManager>>,
    family_id: String,
) -> Result<serde_json::Value, String> {
    let family = family_mode.get_family(&family_id).await?;

    let users: Vec<serde_json::Value> = family
        .users
        .values()
        .map(|u| {
            json!({
                "id": u.id,
                "name": u.name,
                "email": u.email,
                "role": u.role.to_string(),
                "created_at": u.created_at,
                "last_active": u.last_active,
                "is_active": u.is_active,
            })
        })
        .collect();

    Ok(json!({
        "id": family.id,
        "name": family.name,
        "description": family.description,
        "admin_id": family.admin_id,
        "users": users,
        "user_count": family.users.len(),
        "created_at": family.created_at,
        "max_users": family.max_users,
    }))
}

#[tauri::command]
pub async fn list_user_families(
    family_mode: State<'_, std::sync::Arc<FamilyModeManager>>,
    user_id: String,
) -> Result<Vec<serde_json::Value>, String> {
    let families = family_mode.list_user_families(&user_id).await;

    let result = families
        .iter()
        .map(|f| {
            json!({
                "id": f.id,
                "name": f.name,
                "admin_id": f.admin_id,
                "user_count": f.users.len(),
                "created_at": f.created_at,
            })
        })
        .collect();

    Ok(result)
}

#[tauri::command]
pub async fn send_family_invitation(
    family_mode: State<'_, std::sync::Arc<FamilyModeManager>>,
    family_id: String,
    caller_id: String, // ID of the authenticated caller performing the invitation
    email: String,
    role: String,
) -> Result<String, String> {
    info!(
        "[FAMILY] User {} sending invitation to {} for family {}",
        caller_id, email, family_id
    );

    let user_role = match role.as_str() {
        "admin" => UserRole::Admin,
        "parent" => UserRole::Parent,
        "user" => UserRole::User,
        "guest" => UserRole::Guest,
        _ => return Err("Invalid role".to_string()),
    };

    family_mode
        .send_invitation(&family_id, &caller_id, email, user_role)
        .await
}

#[tauri::command]
pub async fn accept_family_invitation(
    family_mode: State<'_, std::sync::Arc<FamilyModeManager>>,
    invitation_id: String,
    invitation_code: String, // Short code from the invitation link — must match stored code
    user_id: String,
    user_name: String,
    user_email: String, // Must match the email the invitation was sent to
) -> Result<String, String> {
    info!("[FAMILY] User {} accepting invitation", user_id);
    family_mode
        .accept_invitation(
            &invitation_id,
            &invitation_code,
            user_id,
            user_name,
            &user_email,
        )
        .await
}

#[tauri::command]
pub async fn remove_family_user(
    family_mode: State<'_, std::sync::Arc<FamilyModeManager>>,
    family_id: String,
    caller_id: String, // ID of the authenticated caller performing the removal
    user_id: String,
) -> Result<(), String> {
    info!(
        "[FAMILY] User {} removing user {} from family {}",
        caller_id, user_id, family_id
    );
    family_mode
        .remove_user(&family_id, &caller_id, &user_id)
        .await
}

#[tauri::command]
pub async fn update_family_user_role(
    family_mode: State<'_, std::sync::Arc<FamilyModeManager>>,
    family_id: String,
    caller_id: String, // ID of the authenticated caller performing the role change
    user_id: String,
    new_role: String,
) -> Result<(), String> {
    info!(
        "[FAMILY] User {} updating role for user {} in family {}",
        caller_id, user_id, family_id
    );

    let role = match new_role.as_str() {
        "admin" => UserRole::Admin,
        "parent" => UserRole::Parent,
        "user" => UserRole::User,
        "guest" => UserRole::Guest,
        _ => return Err("Invalid role".to_string()),
    };

    family_mode
        .update_user_role(&family_id, &caller_id, &user_id, role)
        .await
}

#[tauri::command]
pub async fn get_pending_invitations(
    family_mode: State<'_, std::sync::Arc<FamilyModeManager>>,
    email: String,
) -> Result<Vec<serde_json::Value>, String> {
    let invitations = family_mode.get_pending_invitations(&email).await;

    let result = invitations
        .iter()
        .map(|inv| {
            json!({
                "id": inv.id,
                "family_id": inv.family_id,
                "role": inv.role.to_string(),
                "created_at": inv.created_at,
                "expires_at": inv.expires_at,
            })
        })
        .collect();

    Ok(result)
}

#[tauri::command]
pub async fn create_shared_memory_pool(
    family_mode: State<'_, std::sync::Arc<FamilyModeManager>>,
    family_id: String,
    access_level: String,
) -> Result<String, String> {
    info!("[FAMILY] Creating memory pool for family {}", family_id);

    let level = match access_level.as_str() {
        "read" => MemoryAccessLevel::Read,
        "write" => MemoryAccessLevel::Write,
        "admin" => MemoryAccessLevel::Admin,
        _ => return Err("Invalid access level".to_string()),
    };

    family_mode.create_memory_pool(&family_id, level).await
}

#[tauri::command]
pub async fn get_family_activity(
    family_mode: State<'_, std::sync::Arc<FamilyModeManager>>,
    family_id: String,
    limit: Option<usize>,
) -> Result<Vec<serde_json::Value>, String> {
    let limit = limit.unwrap_or(100);
    let activity = family_mode.get_family_activity(&family_id, limit).await?;

    let result = activity
        .iter()
        .map(|entry| {
            json!({
                "timestamp": entry.timestamp,
                "user_id": entry.user_id,
                "action": entry.action,
                "details": entry.details,
            })
        })
        .collect();

    Ok(result)
}

#[tauri::command]
pub async fn get_user_activity(
    family_mode: State<'_, std::sync::Arc<FamilyModeManager>>,
    user_id: String,
    limit: Option<usize>,
) -> Result<Vec<serde_json::Value>, String> {
    let limit = limit.unwrap_or(100);
    let activity = family_mode.get_user_activity(&user_id, limit).await;

    let result = activity
        .iter()
        .map(|entry| {
            json!({
                "timestamp": entry.timestamp,
                "action": entry.action,
                "details": entry.details,
            })
        })
        .collect();

    Ok(result)
}

#[tauri::command]
pub async fn log_family_activity(
    family_mode: State<'_, std::sync::Arc<FamilyModeManager>>,
    user_id: String,
    action: String,
    details: String,
) -> Result<(), String> {
    family_mode.log_activity(user_id, action, details).await;
    Ok(())
}
