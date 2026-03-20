//! System Monitoring Dashboard
//!
//! Provides comprehensive system metrics, health status, and performance monitoring:
//! - Real-time system resource usage (CPU, memory, disk)
//! - Service health and availability
//! - Database and backup statistics
//! - Voice pipeline metrics
//! - Session and message throughput
//! - API latency and error tracking
//! - Performance trends and alerts

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::RwLock;

/// System resource metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemMetrics {
    /// CPU usage percentage (0-100)
    pub cpu_usage: f32,
    /// Memory usage in bytes
    pub memory_used: u64,
    /// Total available memory in bytes
    pub memory_total: u64,
    /// Disk usage in bytes
    pub disk_used: u64,
    /// Total disk space in bytes
    pub disk_total: u64,
    /// Timestamp of metric collection
    pub timestamp: DateTime<Utc>,
}

/// Service health status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ServiceStatus {
    /// Service is operational
    Healthy,
    /// Service has warnings but functioning
    Degraded,
    /// Service is not responding
    Unhealthy,
    /// Service is not running
    Offline,
}

impl std::fmt::Display for ServiceStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServiceStatus::Healthy => write!(f, "Healthy"),
            ServiceStatus::Degraded => write!(f, "Degraded"),
            ServiceStatus::Unhealthy => write!(f, "Unhealthy"),
            ServiceStatus::Offline => write!(f, "Offline"),
        }
    }
}

/// Service health information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceHealth {
    /// Service name
    pub name: String,
    /// Current status
    pub status: ServiceStatus,
    /// Human-readable status message
    pub message: String,
    /// Response time in milliseconds
    pub response_time_ms: u32,
    /// Last check timestamp
    pub last_check: DateTime<Utc>,
    /// Consecutive failures
    pub failure_count: u32,
    /// Uptime percentage (0-100)
    pub uptime: f32,
}

/// Database statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseStats {
    /// Database name
    pub name: String,
    /// File size in bytes
    pub size_bytes: u64,
    /// Number of tables
    pub table_count: u32,
    /// Estimated row count
    pub row_count: u64,
    /// Last backup timestamp
    pub last_backup: Option<DateTime<Utc>>,
    /// Health status
    pub is_healthy: bool,
    /// Fragmentation percentage (0-100)
    pub fragmentation: f32,
}

/// Voice service metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceMetrics {
    /// Wake word detections today
    pub wakeword_detections: u32,
    /// Successful transcriptions
    pub successful_transcriptions: u32,
    /// Failed transcriptions
    pub failed_transcriptions: u32,
    /// Total TTS generations
    pub tts_generations: u32,
    /// Average STT latency (ms)
    pub avg_stt_latency_ms: u32,
    /// Average TTS latency (ms)
    pub avg_tts_latency_ms: u32,
    /// Voice service status
    pub service_status: ServiceStatus,
}

/// Session and message metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetrics {
    /// Active sessions
    pub active_sessions: u32,
    /// Total sessions today
    pub sessions_today: u32,
    /// Messages sent today
    pub messages_today: u64,
    /// Average message latency (ms)
    pub avg_message_latency_ms: u32,
    /// Messages per minute (last 5 min)
    pub messages_per_minute: f32,
    /// Error rate percentage (0-100)
    pub error_rate: f32,
}

/// API metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiMetrics {
    /// Total API requests today
    pub total_requests: u64,
    /// Successful requests
    pub successful_requests: u64,
    /// Failed requests
    pub failed_requests: u64,
    /// 99th percentile latency (ms)
    pub p99_latency_ms: u32,
    /// Slowest endpoint name
    pub slowest_endpoint: String,
    /// Most requested endpoint
    pub most_requested_endpoint: String,
    /// Cache hit rate (0-100)
    pub cache_hit_rate: f32,
}

/// Alert/warning item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardAlert {
    /// Alert severity (info, warning, error)
    pub severity: String,
    /// Alert message
    pub message: String,
    /// Affected service
    pub service: String,
    /// When alert was raised
    pub timestamp: DateTime<Utc>,
}

/// Comprehensive dashboard data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardData {
    /// System resource metrics
    pub system_metrics: SystemMetrics,
    /// Service health status
    pub services: Vec<ServiceHealth>,
    /// Database statistics
    pub databases: Vec<DatabaseStats>,
    /// Voice metrics
    pub voice_metrics: VoiceMetrics,
    /// Session metrics
    pub session_metrics: SessionMetrics,
    /// API metrics
    pub api_metrics: ApiMetrics,
    /// Active alerts
    pub alerts: Vec<DashboardAlert>,
    /// Dashboard timestamp
    pub timestamp: DateTime<Utc>,
}

/// Metric data point for historical tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricDataPoint {
    pub timestamp: DateTime<Utc>,
    pub value: f32,
}

/// Historical metrics (for graphs/trends)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoricalMetrics {
    /// CPU usage history (last 24 hours)
    pub cpu_history: Vec<MetricDataPoint>,
    /// Memory usage history
    pub memory_history: Vec<MetricDataPoint>,
    /// Message throughput history
    pub message_throughput_history: Vec<MetricDataPoint>,
    /// API latency history
    pub api_latency_history: Vec<MetricDataPoint>,
    /// Error rate history
    pub error_rate_history: Vec<MetricDataPoint>,
}

/// Dashboard monitoring manager
pub struct DashboardManager {
    /// Current system metrics
    system_metrics: Arc<RwLock<Option<SystemMetrics>>>,
    /// Service health status
    service_health: Arc<RwLock<Vec<ServiceHealth>>>,
    /// Active alerts
    alerts: Arc<RwLock<VecDeque<DashboardAlert>>>,
    /// Historical metrics (max 1440 points = 24 hours @ 1 min intervals)
    metric_history: Arc<RwLock<HistoricalMetrics>>,
    /// Last update timestamp
    last_update: Arc<RwLock<Option<DateTime<Utc>>>>,
}

impl DashboardManager {
    /// Create a new dashboard manager
    pub fn new() -> Self {
        Self {
            system_metrics: Arc::new(RwLock::new(None)),
            service_health: Arc::new(RwLock::new(Vec::new())),
            alerts: Arc::new(RwLock::new(VecDeque::new())),
            metric_history: Arc::new(RwLock::new(HistoricalMetrics {
                cpu_history: Vec::new(),
                memory_history: Vec::new(),
                message_throughput_history: Vec::new(),
                api_latency_history: Vec::new(),
                error_rate_history: Vec::new(),
            })),
            last_update: Arc::new(RwLock::new(None)),
        }
    }

    /// Update system metrics
    pub async fn update_system_metrics(&self, metrics: SystemMetrics) -> Result<()> {
        *self.system_metrics.write().await = Some(metrics.clone());

        // Add to history (keep max 1440 points)
        let mut history = self.metric_history.write().await;
        history.cpu_history.push(MetricDataPoint {
            timestamp: metrics.timestamp,
            value: metrics.cpu_usage,
        });
        history.memory_history.push(MetricDataPoint {
            timestamp: metrics.timestamp,
            value: (metrics.memory_used as f32 / metrics.memory_total as f32) * 100.0,
        });

        // Trim old history (keep last 24 hours)
        let cutoff = metrics.timestamp - Duration::hours(24);
        history.cpu_history.retain(|p| p.timestamp > cutoff);
        history.memory_history.retain(|p| p.timestamp > cutoff);

        *self.last_update.write().await = Some(Utc::now());
        Ok(())
    }

    /// Update service health
    pub async fn update_service_health(&self, health: ServiceHealth) -> Result<()> {
        let mut services = self.service_health.write().await;

        // Update existing service or add new one
        if let Some(existing) = services.iter_mut().find(|s| s.name == health.name) {
            *existing = health;
        } else {
            services.push(health);
        }

        *self.last_update.write().await = Some(Utc::now());
        Ok(())
    }

    /// Add alert
    pub async fn add_alert(&self, alert: DashboardAlert) -> Result<()> {
        let mut alerts = self.alerts.write().await;
        alerts.push_back(alert);

        // Keep max 100 alerts
        while alerts.len() > 100 {
            alerts.pop_front();
        }

        *self.last_update.write().await = Some(Utc::now());
        Ok(())
    }

    /// Record message throughput
    pub async fn record_message_throughput(&self, throughput: f32) -> Result<()> {
        let mut history = self.metric_history.write().await;
        history.message_throughput_history.push(MetricDataPoint {
            timestamp: Utc::now(),
            value: throughput,
        });

        // Trim old history
        let cutoff = Utc::now() - Duration::hours(24);
        history
            .message_throughput_history
            .retain(|p| p.timestamp > cutoff);

        Ok(())
    }

    /// Record API latency
    pub async fn record_api_latency(&self, latency_ms: u32) -> Result<()> {
        let mut history = self.metric_history.write().await;
        history.api_latency_history.push(MetricDataPoint {
            timestamp: Utc::now(),
            value: latency_ms as f32,
        });

        // Trim old history
        let cutoff = Utc::now() - Duration::hours(24);
        history.api_latency_history.retain(|p| p.timestamp > cutoff);

        Ok(())
    }

    /// Record error rate
    pub async fn record_error_rate(&self, error_rate: f32) -> Result<()> {
        let mut history = self.metric_history.write().await;
        history.error_rate_history.push(MetricDataPoint {
            timestamp: Utc::now(),
            value: error_rate,
        });

        // Trim old history
        let cutoff = Utc::now() - Duration::hours(24);
        history.error_rate_history.retain(|p| p.timestamp > cutoff);

        Ok(())
    }

    /// Get current dashboard data
    pub async fn get_dashboard_data(
        &self,
        voice_metrics: Option<VoiceMetrics>,
        session_metrics: Option<SessionMetrics>,
        api_metrics: Option<ApiMetrics>,
        databases: Option<Vec<DatabaseStats>>,
    ) -> DashboardData {
        let system_metrics = self
            .system_metrics
            .read()
            .await
            .clone()
            .unwrap_or(SystemMetrics {
                cpu_usage: 0.0,
                memory_used: 0,
                memory_total: 1,
                disk_used: 0,
                disk_total: 1,
                timestamp: Utc::now(),
            });

        let services = self.service_health.read().await.clone();
        let alerts = self.alerts.read().await.iter().cloned().collect();

        DashboardData {
            system_metrics,
            services,
            databases: databases.unwrap_or_default(),
            voice_metrics: voice_metrics.unwrap_or(VoiceMetrics {
                wakeword_detections: 0,
                successful_transcriptions: 0,
                failed_transcriptions: 0,
                tts_generations: 0,
                avg_stt_latency_ms: 0,
                avg_tts_latency_ms: 0,
                service_status: ServiceStatus::Offline,
            }),
            session_metrics: session_metrics.unwrap_or(SessionMetrics {
                active_sessions: 0,
                sessions_today: 0,
                messages_today: 0,
                avg_message_latency_ms: 0,
                messages_per_minute: 0.0,
                error_rate: 0.0,
            }),
            api_metrics: api_metrics.unwrap_or(ApiMetrics {
                total_requests: 0,
                successful_requests: 0,
                failed_requests: 0,
                p99_latency_ms: 0,
                slowest_endpoint: "N/A".to_string(),
                most_requested_endpoint: "N/A".to_string(),
                cache_hit_rate: 0.0,
            }),
            alerts,
            timestamp: Utc::now(),
        }
    }

    /// Get historical metrics
    pub async fn get_historical_metrics(&self) -> HistoricalMetrics {
        self.metric_history.read().await.clone()
    }

    /// Get service health
    pub async fn get_service_health(&self) -> Vec<ServiceHealth> {
        self.service_health.read().await.clone()
    }

    /// Clear old alerts (older than specified duration)
    pub async fn clear_old_alerts(&self, older_than_hours: i64) -> usize {
        let mut alerts = self.alerts.write().await;
        let cutoff = Utc::now() - Duration::hours(older_than_hours);
        let original_count = alerts.len();
        alerts.retain(|a| a.timestamp > cutoff);
        original_count - alerts.len()
    }

    /// Get last update time
    #[allow(dead_code)]
    pub async fn get_last_update(&self) -> Option<DateTime<Utc>> {
        self.last_update.read().await.clone()
    }
}

impl Default for DashboardManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_dashboard_manager_creation() {
        let manager = DashboardManager::new();
        let data = manager.get_dashboard_data(None, None, None, None).await;
        assert_eq!(data.services.len(), 0);
    }

    #[tokio::test]
    async fn test_service_health_update() {
        let manager = DashboardManager::new();
        let health = ServiceHealth {
            name: "Test Service".to_string(),
            status: ServiceStatus::Healthy,
            message: "All good".to_string(),
            response_time_ms: 10,
            last_check: Utc::now(),
            failure_count: 0,
            uptime: 100.0,
        };

        manager.update_service_health(health.clone()).await.unwrap();
        let services = manager.get_service_health().await;
        assert_eq!(services.len(), 1);
        assert_eq!(services[0].name, "Test Service");
    }

    #[tokio::test]
    async fn test_alert_management() {
        let manager = DashboardManager::new();
        let alert = DashboardAlert {
            severity: "warning".to_string(),
            message: "Test alert".to_string(),
            service: "Test".to_string(),
            timestamp: Utc::now(),
        };

        manager.add_alert(alert).await.unwrap();
        let data = manager.get_dashboard_data(None, None, None, None).await;
        assert_eq!(data.alerts.len(), 1);
    }
}
