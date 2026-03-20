//! Tauri commands for database maintenance and backup operations

use crate::db_maintenance::DbMaintenanceManager;
use serde_json::json;
use tauri::State;
use tracing::info;

#[tauri::command]
pub async fn create_backup(
    db_maintenance: State<'_, std::sync::Arc<DbMaintenanceManager>>,
    name: Option<String>,
) -> Result<serde_json::Value, String> {
    info!("[DB_MAINTENANCE_CMD] Creating backup: {:?}", name);

    match db_maintenance.create_backup(name).await {
        Ok(metadata) => Ok(json!({
            "id": metadata.id,
            "name": metadata.name,
            "backup_type": metadata.backup_type,
            "created_at": metadata.created_at,
            "total_size": metadata.total_size,
            "databases": metadata.databases,
            "verified": metadata.verified,
        })),
        Err(e) => Err(format!("Failed to create backup: {}", e)),
    }
}

#[tauri::command]
pub async fn restore_backup(
    db_maintenance: State<'_, std::sync::Arc<DbMaintenanceManager>>,
    backup_id: String,
) -> Result<(), String> {
    info!("[DB_MAINTENANCE_CMD] Restoring backup: {}", backup_id);

    db_maintenance
        .restore_backup(&backup_id)
        .await
        .map_err(|e| format!("Failed to restore backup: {}", e))
}

#[tauri::command]
pub async fn list_backups(
    db_maintenance: State<'_, std::sync::Arc<DbMaintenanceManager>>,
) -> Result<Vec<serde_json::Value>, String> {
    match db_maintenance.list_backups().await {
        Ok(backups) => {
            let result = backups
                .iter()
                .map(|b| {
                    json!({
                        "id": b.id,
                        "name": b.name,
                        "backup_type": b.backup_type,
                        "created_at": b.created_at,
                        "total_size": b.total_size,
                        "databases": b.databases,
                        "verified": b.verified,
                    })
                })
                .collect();

            Ok(result)
        }
        Err(e) => Err(format!("Failed to list backups: {}", e)),
    }
}

#[tauri::command]
pub async fn delete_backup(
    db_maintenance: State<'_, std::sync::Arc<DbMaintenanceManager>>,
    backup_id: String,
) -> Result<(), String> {
    info!("[DB_MAINTENANCE_CMD] Deleting backup: {}", backup_id);

    db_maintenance
        .delete_backup(&backup_id)
        .await
        .map_err(|e| format!("Failed to delete backup: {}", e))
}

#[tauri::command]
pub async fn verify_backup(
    db_maintenance: State<'_, std::sync::Arc<DbMaintenanceManager>>,
    backup_id: String,
) -> Result<bool, String> {
    info!("[DB_MAINTENANCE_CMD] Verifying backup: {}", backup_id);

    db_maintenance
        .verify_backup(&backup_id)
        .await
        .map_err(|e| format!("Failed to verify backup: {}", e))
}

#[tauri::command]
pub async fn perform_health_check(
    db_maintenance: State<'_, std::sync::Arc<DbMaintenanceManager>>,
) -> Result<Vec<serde_json::Value>, String> {
    info!("[DB_MAINTENANCE_CMD] Performing health check");

    match db_maintenance.health_check().await {
        Ok(results) => {
            let result = results
                .iter()
                .map(|r| {
                    json!({
                        "path": r.path,
                        "is_healthy": r.is_healthy,
                        "status": r.status,
                        "error_count": r.error_count,
                        "file_size": r.file_size,
                        "page_count": r.page_count,
                        "checked_at": r.checked_at,
                    })
                })
                .collect();

            Ok(result)
        }
        Err(e) => Err(format!("Failed to perform health check: {}", e)),
    }
}

#[tauri::command]
pub async fn optimize_database(
    db_maintenance: State<'_, std::sync::Arc<DbMaintenanceManager>>,
    db_path: String,
) -> Result<String, String> {
    info!("[DB_MAINTENANCE_CMD] Optimizing database: {}", db_path);

    let path = std::path::Path::new(&db_path);
    db_maintenance
        .optimize_database(path)
        .await
        .map_err(|e| format!("Failed to optimize database: {}", e))
}

#[tauri::command]
pub async fn get_maintenance_config(
    db_maintenance: State<'_, std::sync::Arc<DbMaintenanceManager>>,
) -> Result<serde_json::Value, String> {
    let config = db_maintenance.get_config().await;

    Ok(json!({
        "auto_backup_enabled": config.auto_backup_enabled,
        "auto_backup_interval_minutes": config.auto_backup_interval_minutes,
        "max_backups": config.max_backups,
        "backup_retention_days": config.backup_retention_days,
        "auto_vacuum_enabled": config.auto_vacuum_enabled,
        "auto_analyze_enabled": config.auto_analyze_enabled,
        "health_check_enabled": config.health_check_enabled,
        "health_check_interval_hours": config.health_check_interval_hours,
        "compression_enabled": config.compression_enabled,
    }))
}

#[tauri::command]
pub async fn update_maintenance_config(
    db_maintenance: State<'_, std::sync::Arc<DbMaintenanceManager>>,
    auto_backup_enabled: Option<bool>,
    auto_backup_interval_minutes: Option<u32>,
    max_backups: Option<usize>,
    backup_retention_days: Option<u32>,
    auto_vacuum_enabled: Option<bool>,
    auto_analyze_enabled: Option<bool>,
    health_check_enabled: Option<bool>,
    health_check_interval_hours: Option<u32>,
    compression_enabled: Option<bool>,
) -> Result<(), String> {
    let mut config = db_maintenance.get_config().await;

    if let Some(v) = auto_backup_enabled {
        config.auto_backup_enabled = v;
    }
    if let Some(v) = auto_backup_interval_minutes {
        config.auto_backup_interval_minutes = v;
    }
    if let Some(v) = max_backups {
        config.max_backups = v;
    }
    if let Some(v) = backup_retention_days {
        config.backup_retention_days = v;
    }
    if let Some(v) = auto_vacuum_enabled {
        config.auto_vacuum_enabled = v;
    }
    if let Some(v) = auto_analyze_enabled {
        config.auto_analyze_enabled = v;
    }
    if let Some(v) = health_check_enabled {
        config.health_check_enabled = v;
    }
    if let Some(v) = health_check_interval_hours {
        config.health_check_interval_hours = v;
    }
    if let Some(v) = compression_enabled {
        config.compression_enabled = v;
    }

    db_maintenance
        .update_config(config)
        .await
        .map_err(|e| format!("Failed to update config: {}", e))
}

#[tauri::command]
pub async fn get_backup_stats(
    db_maintenance: State<'_, std::sync::Arc<DbMaintenanceManager>>,
) -> Result<serde_json::Value, String> {
    match db_maintenance.list_backups().await {
        Ok(backups) => {
            let total_backups = backups.len();
            let total_size: u64 = backups.iter().map(|b| b.total_size).sum();
            let last_backup = db_maintenance.get_last_backup_time().await;
            let last_health_check = db_maintenance.get_last_health_check_time().await;

            Ok(json!({
                "total_backups": total_backups,
                "total_size": total_size,
                "last_backup_time": last_backup,
                "last_health_check_time": last_health_check,
                "backups": backups.iter().map(|b| {
                    json!({
                        "id": b.id,
                        "name": b.name,
                        "created_at": b.created_at,
                        "size": b.total_size,
                    })
                }).collect::<Vec<_>>(),
            }))
        }
        Err(e) => Err(format!("Failed to get backup stats: {}", e)),
    }
}
