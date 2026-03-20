//! Tauri commands for system monitoring dashboard

use crate::dashboard::{
    DashboardAlert, DashboardManager, ServiceHealth, ServiceStatus, SystemMetrics,
};
use chrono::Utc;
use serde_json::json;
use tauri::State;
use tracing::info;

#[tauri::command]
pub async fn get_dashboard_data(
    dashboard: State<'_, std::sync::Arc<DashboardManager>>,
) -> Result<serde_json::Value, String> {
    info!("[DASHBOARD] Getting dashboard data");

    // In a full implementation, these would be collected from various services
    let data = dashboard.get_dashboard_data(None, None, None, None).await;

    Ok(json!({
        "system_metrics": {
            "cpu_usage": data.system_metrics.cpu_usage,
            "memory_used": data.system_metrics.memory_used,
            "memory_total": data.system_metrics.memory_total,
            "memory_used_percent": (data.system_metrics.memory_used as f32 / data.system_metrics.memory_total as f32) * 100.0,
            "disk_used": data.system_metrics.disk_used,
            "disk_total": data.system_metrics.disk_total,
            "disk_used_percent": (data.system_metrics.disk_used as f32 / data.system_metrics.disk_total as f32) * 100.0,
            "timestamp": data.system_metrics.timestamp,
        },
        "services": data.services.iter().map(|s| {
            json!({
                "name": s.name,
                "status": s.status.to_string(),
                "message": s.message,
                "response_time_ms": s.response_time_ms,
                "last_check": s.last_check,
                "failure_count": s.failure_count,
                "uptime": s.uptime,
            })
        }).collect::<Vec<_>>(),
        "voice_metrics": {
            "wakeword_detections": data.voice_metrics.wakeword_detections,
            "successful_transcriptions": data.voice_metrics.successful_transcriptions,
            "failed_transcriptions": data.voice_metrics.failed_transcriptions,
            "tts_generations": data.voice_metrics.tts_generations,
            "avg_stt_latency_ms": data.voice_metrics.avg_stt_latency_ms,
            "avg_tts_latency_ms": data.voice_metrics.avg_tts_latency_ms,
            "service_status": data.voice_metrics.service_status.to_string(),
        },
        "session_metrics": {
            "active_sessions": data.session_metrics.active_sessions,
            "sessions_today": data.session_metrics.sessions_today,
            "messages_today": data.session_metrics.messages_today,
            "avg_message_latency_ms": data.session_metrics.avg_message_latency_ms,
            "messages_per_minute": data.session_metrics.messages_per_minute,
            "error_rate": data.session_metrics.error_rate,
        },
        "api_metrics": {
            "total_requests": data.api_metrics.total_requests,
            "successful_requests": data.api_metrics.successful_requests,
            "failed_requests": data.api_metrics.failed_requests,
            "p99_latency_ms": data.api_metrics.p99_latency_ms,
            "slowest_endpoint": data.api_metrics.slowest_endpoint,
            "most_requested_endpoint": data.api_metrics.most_requested_endpoint,
            "cache_hit_rate": data.api_metrics.cache_hit_rate,
        },
        "databases": data.databases.iter().map(|db| {
            json!({
                "name": db.name,
                "size_bytes": db.size_bytes,
                "table_count": db.table_count,
                "row_count": db.row_count,
                "last_backup": db.last_backup,
                "is_healthy": db.is_healthy,
                "fragmentation": db.fragmentation,
            })
        }).collect::<Vec<_>>(),
        "alerts": data.alerts.iter().map(|a| {
            json!({
                "severity": a.severity,
                "message": a.message,
                "service": a.service,
                "timestamp": a.timestamp,
            })
        }).collect::<Vec<_>>(),
        "timestamp": data.timestamp,
    }))
}

#[tauri::command]
pub async fn update_system_metrics(
    dashboard: State<'_, std::sync::Arc<DashboardManager>>,
    cpu_usage: f32,
    memory_used: u64,
    memory_total: u64,
    disk_used: u64,
    disk_total: u64,
) -> Result<(), String> {
    let metrics = SystemMetrics {
        cpu_usage,
        memory_used,
        memory_total,
        disk_used,
        disk_total,
        timestamp: Utc::now(),
    };

    dashboard
        .update_system_metrics(metrics)
        .await
        .map_err(|e| format!("Failed to update metrics: {}", e))
}

#[tauri::command]
pub async fn update_service_health(
    dashboard: State<'_, std::sync::Arc<DashboardManager>>,
    name: String,
    status: String,
    message: String,
    response_time_ms: u32,
) -> Result<(), String> {
    let service_status = match status.as_str() {
        "healthy" => ServiceStatus::Healthy,
        "degraded" => ServiceStatus::Degraded,
        "unhealthy" => ServiceStatus::Unhealthy,
        "offline" => ServiceStatus::Offline,
        _ => ServiceStatus::Offline,
    };

    let health = ServiceHealth {
        name,
        status: service_status,
        message,
        response_time_ms,
        last_check: Utc::now(),
        failure_count: 0,
        uptime: 100.0,
    };

    dashboard
        .update_service_health(health)
        .await
        .map_err(|e| format!("Failed to update service health: {}", e))
}

#[tauri::command]
pub async fn add_dashboard_alert(
    dashboard: State<'_, std::sync::Arc<DashboardManager>>,
    severity: String,
    message: String,
    service: String,
) -> Result<(), String> {
    let alert = DashboardAlert {
        severity,
        message,
        service,
        timestamp: Utc::now(),
    };

    dashboard
        .add_alert(alert)
        .await
        .map_err(|e| format!("Failed to add alert: {}", e))
}

#[tauri::command]
pub async fn record_message_throughput(
    dashboard: State<'_, std::sync::Arc<DashboardManager>>,
    throughput: f32,
) -> Result<(), String> {
    dashboard
        .record_message_throughput(throughput)
        .await
        .map_err(|e| format!("Failed to record throughput: {}", e))
}

#[tauri::command]
pub async fn record_api_latency(
    dashboard: State<'_, std::sync::Arc<DashboardManager>>,
    latency_ms: u32,
) -> Result<(), String> {
    dashboard
        .record_api_latency(latency_ms)
        .await
        .map_err(|e| format!("Failed to record latency: {}", e))
}

#[tauri::command]
pub async fn record_error_rate(
    dashboard: State<'_, std::sync::Arc<DashboardManager>>,
    error_rate: f32,
) -> Result<(), String> {
    dashboard
        .record_error_rate(error_rate)
        .await
        .map_err(|e| format!("Failed to record error rate: {}", e))
}

#[tauri::command]
pub async fn get_historical_metrics(
    dashboard: State<'_, std::sync::Arc<DashboardManager>>,
) -> Result<serde_json::Value, String> {
    let history = dashboard.get_historical_metrics().await;

    Ok(json!({
        "cpu_history": history.cpu_history.iter().map(|p| {
            json!({
                "timestamp": p.timestamp,
                "value": p.value,
            })
        }).collect::<Vec<_>>(),
        "memory_history": history.memory_history.iter().map(|p| {
            json!({
                "timestamp": p.timestamp,
                "value": p.value,
            })
        }).collect::<Vec<_>>(),
        "message_throughput_history": history.message_throughput_history.iter().map(|p| {
            json!({
                "timestamp": p.timestamp,
                "value": p.value,
            })
        }).collect::<Vec<_>>(),
        "api_latency_history": history.api_latency_history.iter().map(|p| {
            json!({
                "timestamp": p.timestamp,
                "value": p.value,
            })
        }).collect::<Vec<_>>(),
        "error_rate_history": history.error_rate_history.iter().map(|p| {
            json!({
                "timestamp": p.timestamp,
                "value": p.value,
            })
        }).collect::<Vec<_>>(),
    }))
}

#[tauri::command]
pub async fn get_service_health(
    dashboard: State<'_, std::sync::Arc<DashboardManager>>,
) -> Result<Vec<serde_json::Value>, String> {
    let services = dashboard.get_service_health().await;

    Ok(services
        .iter()
        .map(|s| {
            json!({
                "name": s.name,
                "status": s.status.to_string(),
                "message": s.message,
                "response_time_ms": s.response_time_ms,
                "last_check": s.last_check,
                "failure_count": s.failure_count,
                "uptime": s.uptime,
            })
        })
        .collect())
}

#[tauri::command]
pub async fn clear_old_alerts(
    dashboard: State<'_, std::sync::Arc<DashboardManager>>,
    older_than_hours: i64,
) -> Result<usize, String> {
    Ok(dashboard.clear_old_alerts(older_than_hours).await)
}
