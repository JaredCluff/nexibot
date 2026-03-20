//! Gateway connection metrics and monitoring.
//!
//! Collects per-connection and global metrics for the WebSocket gateway,
//! including bytes transferred, message counts, and error rates.
#![allow(dead_code)]

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Per-connection metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionMetrics {
    /// Total bytes sent to this connection.
    pub bytes_sent: u64,
    /// Total bytes received from this connection.
    pub bytes_received: u64,
    /// Number of messages sent to this connection.
    pub messages_sent: u64,
    /// Number of messages received from this connection.
    pub messages_received: u64,
    /// Number of errors on this connection.
    pub errors: u64,
    /// When the connection was established.
    pub connected_at: DateTime<Utc>,
    /// Timestamp of the most recent activity.
    pub last_activity: DateTime<Utc>,
}

impl ConnectionMetrics {
    /// Create a new metrics instance for a connection established now.
    fn new() -> Self {
        let now = Utc::now();
        Self {
            bytes_sent: 0,
            bytes_received: 0,
            messages_sent: 0,
            messages_received: 0,
            errors: 0,
            connected_at: now,
            last_activity: now,
        }
    }
}

/// Aggregated summary across all connections.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSummary {
    /// Number of connections currently tracked.
    pub total_connections: usize,
    /// Global bytes sent across all connections.
    pub total_bytes_sent: u64,
    /// Global bytes received across all connections.
    pub total_bytes_received: u64,
    /// Total errors across all connections.
    pub total_errors: u64,
}

/// Collects and manages metrics for all gateway connections.
#[derive(Debug)]
pub struct MetricsCollector {
    per_connection: HashMap<String, ConnectionMetrics>,
    global_bytes_sent: u64,
    global_bytes_received: u64,
}

impl MetricsCollector {
    /// Create a new, empty metrics collector.
    pub fn new() -> Self {
        Self {
            per_connection: HashMap::new(),
            global_bytes_sent: 0,
            global_bytes_received: 0,
        }
    }

    /// Ensure a metrics entry exists for the given connection.
    fn ensure_connection(&mut self, connection_id: &str) {
        self.per_connection
            .entry(connection_id.to_string())
            .or_insert_with(ConnectionMetrics::new);
    }

    /// Record bytes sent on a connection.
    pub fn record_sent(&mut self, connection_id: &str, bytes: u64) {
        self.ensure_connection(connection_id);
        if let Some(metrics) = self.per_connection.get_mut(connection_id) {
            metrics.bytes_sent += bytes;
            metrics.messages_sent += 1;
            metrics.last_activity = Utc::now();
        }
        self.global_bytes_sent += bytes;
    }

    /// Record bytes received on a connection.
    pub fn record_received(&mut self, connection_id: &str, bytes: u64) {
        self.ensure_connection(connection_id);
        if let Some(metrics) = self.per_connection.get_mut(connection_id) {
            metrics.bytes_received += bytes;
            metrics.messages_received += 1;
            metrics.last_activity = Utc::now();
        }
        self.global_bytes_received += bytes;
    }

    /// Record an error on a connection.
    pub fn record_error(&mut self, connection_id: &str) {
        self.ensure_connection(connection_id);
        if let Some(metrics) = self.per_connection.get_mut(connection_id) {
            metrics.errors += 1;
            metrics.last_activity = Utc::now();
        }
    }

    /// Get the metrics for a specific connection.
    pub fn get_connection_metrics(&self, connection_id: &str) -> Option<&ConnectionMetrics> {
        self.per_connection.get(connection_id)
    }

    /// Build a summary of all tracked metrics.
    pub fn get_summary(&self) -> MetricsSummary {
        let total_errors: u64 = self.per_connection.values().map(|m| m.errors).sum();

        MetricsSummary {
            total_connections: self.per_connection.len(),
            total_bytes_sent: self.global_bytes_sent,
            total_bytes_received: self.global_bytes_received,
            total_errors,
        }
    }

    /// Remove metrics for a disconnected connection.
    pub fn remove_connection(&mut self, connection_id: &str) {
        self.per_connection.remove(connection_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_collector_empty() {
        let collector = MetricsCollector::new();
        let summary = collector.get_summary();
        assert_eq!(summary.total_connections, 0);
        assert_eq!(summary.total_bytes_sent, 0);
        assert_eq!(summary.total_bytes_received, 0);
        assert_eq!(summary.total_errors, 0);
    }

    #[test]
    fn test_record_sent() {
        let mut collector = MetricsCollector::new();
        collector.record_sent("conn-1", 100);
        collector.record_sent("conn-1", 200);

        let metrics = collector.get_connection_metrics("conn-1").unwrap();
        assert_eq!(metrics.bytes_sent, 300);
        assert_eq!(metrics.messages_sent, 2);
        assert_eq!(metrics.bytes_received, 0);

        let summary = collector.get_summary();
        assert_eq!(summary.total_bytes_sent, 300);
    }

    #[test]
    fn test_record_received() {
        let mut collector = MetricsCollector::new();
        collector.record_received("conn-1", 50);
        collector.record_received("conn-1", 75);

        let metrics = collector.get_connection_metrics("conn-1").unwrap();
        assert_eq!(metrics.bytes_received, 125);
        assert_eq!(metrics.messages_received, 2);

        let summary = collector.get_summary();
        assert_eq!(summary.total_bytes_received, 125);
    }

    #[test]
    fn test_record_error() {
        let mut collector = MetricsCollector::new();
        collector.record_error("conn-1");
        collector.record_error("conn-1");
        collector.record_error("conn-2");

        let m1 = collector.get_connection_metrics("conn-1").unwrap();
        assert_eq!(m1.errors, 2);

        let m2 = collector.get_connection_metrics("conn-2").unwrap();
        assert_eq!(m2.errors, 1);

        let summary = collector.get_summary();
        assert_eq!(summary.total_errors, 3);
    }

    #[test]
    fn test_multiple_connections() {
        let mut collector = MetricsCollector::new();
        collector.record_sent("conn-a", 100);
        collector.record_sent("conn-b", 200);
        collector.record_received("conn-a", 50);

        let summary = collector.get_summary();
        assert_eq!(summary.total_connections, 2);
        assert_eq!(summary.total_bytes_sent, 300);
        assert_eq!(summary.total_bytes_received, 50);
    }

    #[test]
    fn test_remove_connection() {
        let mut collector = MetricsCollector::new();
        collector.record_sent("conn-1", 100);
        assert!(collector.get_connection_metrics("conn-1").is_some());

        collector.remove_connection("conn-1");
        assert!(collector.get_connection_metrics("conn-1").is_none());

        let summary = collector.get_summary();
        assert_eq!(summary.total_connections, 0);
        // Global counters persist even after removal
        assert_eq!(summary.total_bytes_sent, 100);
    }

    #[test]
    fn test_get_nonexistent_connection() {
        let collector = MetricsCollector::new();
        assert!(collector.get_connection_metrics("no-such").is_none());
    }

    #[test]
    fn test_metrics_summary_serde() {
        let summary = MetricsSummary {
            total_connections: 3,
            total_bytes_sent: 1024,
            total_bytes_received: 2048,
            total_errors: 1,
        };
        let json = serde_json::to_string(&summary).unwrap();
        let deserialized: MetricsSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.total_connections, 3);
        assert_eq!(deserialized.total_bytes_sent, 1024);
        assert_eq!(deserialized.total_bytes_received, 2048);
        assert_eq!(deserialized.total_errors, 1);
    }

    #[test]
    fn test_connection_metrics_serde() {
        let mut collector = MetricsCollector::new();
        collector.record_sent("conn-1", 500);
        collector.record_received("conn-1", 250);
        collector.record_error("conn-1");

        let metrics = collector.get_connection_metrics("conn-1").unwrap();
        let json = serde_json::to_string(metrics).unwrap();
        let deserialized: ConnectionMetrics = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.bytes_sent, 500);
        assert_eq!(deserialized.bytes_received, 250);
        assert_eq!(deserialized.errors, 1);
    }
}
