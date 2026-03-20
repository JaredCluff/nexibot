import React, { useEffect, useState, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import styles from './Dashboard.module.css';
import SystemMetrics from './Dashboard/SystemMetrics';
import ServiceHealth from './Dashboard/ServiceHealth';
import VoiceMetrics from './Dashboard/VoiceMetrics';
import SessionMetrics from './Dashboard/SessionMetrics';
import ApiMetrics from './Dashboard/ApiMetrics';
import AlertsPanel from './Dashboard/AlertsPanel';
import HistoricalChart from './Dashboard/HistoricalChart';
import { ConnectorWizard } from './ConnectorWizard';

interface DashboardData {
  system_metrics: {
    cpu_usage: number;
    memory_used: number;
    memory_total: number;
    memory_used_percent: number;
    disk_used: number;
    disk_total: number;
    disk_used_percent: number;
    timestamp: string;
  };
  services: Array<{
    name: string;
    status: string;
    message: string;
    response_time_ms: number;
    last_check: string;
    failure_count: number;
    uptime: number;
  }>;
  voice_metrics: {
    wakeword_detections: number;
    successful_transcriptions: number;
    failed_transcriptions: number;
    tts_generations: number;
    avg_stt_latency_ms: number;
    avg_tts_latency_ms: number;
    service_status: string;
  };
  session_metrics: {
    active_sessions: number;
    sessions_today: number;
    messages_today: number;
    avg_message_latency_ms: number;
    messages_per_minute: number;
    error_rate: number;
  };
  api_metrics: {
    total_requests: number;
    successful_requests: number;
    failed_requests: number;
    p99_latency_ms: number;
    slowest_endpoint: string;
    most_requested_endpoint: string;
    cache_hit_rate: number;
  };
  databases: Array<{
    name: string;
    size_bytes: number;
    table_count: number;
    row_count: number;
    last_backup: string | null;
    is_healthy: boolean;
    fragmentation: number;
  }>;
  alerts: Array<{
    severity: string;
    message: string;
    service: string;
    timestamp: string;
  }>;
  timestamp: string;
}

interface HistoricalData {
  cpu_history: Array<{ timestamp: string; value: number }>;
  memory_history: Array<{ timestamp: string; value: number }>;
  message_throughput_history: Array<{ timestamp: string; value: number }>;
  api_latency_history: Array<{ timestamp: string; value: number }>;
  error_rate_history: Array<{ timestamp: string; value: number }>;
}

export default function Dashboard() {
  const [dashboardData, setDashboardData] = useState<DashboardData | null>(null);
  const [historicalData, setHistoricalData] = useState<HistoricalData | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [autoRefresh, setAutoRefresh] = useState(true);
  const [showConnectorWizard, setShowConnectorWizard] = useState(false);
  const [hasConnectors, setHasConnectors] = useState<boolean | null>(null);

  const fetchDashboardData = useCallback(async (signal?: { aborted: boolean }) => {
    try {
      const data = await invoke<DashboardData>('get_dashboard_data');
      if (signal?.aborted) return;
      setDashboardData(data);
      setError(null);
    } catch (err) {
      if (signal?.aborted) return;
      setError(String(err));
    } finally {
      if (!signal?.aborted) setLoading(false);
    }
  }, []);

  const fetchHistoricalData = useCallback(async (signal?: { aborted: boolean }) => {
    try {
      const data = await invoke<HistoricalData>('get_historical_metrics');
      if (signal?.aborted) return;
      setHistoricalData(data);
    } catch {
      // Historical data is supplementary; silently ignore failures
    }
  }, []);

  // Check whether user has any KN connectors (show CTA if not)
  useEffect(() => {
    invoke<unknown[]>('list_user_connectors')
      .then((cs) => setHasConnectors(cs.length > 0))
      .catch(() => setHasConnectors(null)); // not authenticated — hide CTA
  }, [showConnectorWizard]);

  useEffect(() => {
    const signal = { aborted: false };
    fetchDashboardData(signal);
    fetchHistoricalData(signal);

    // Auto-refresh dashboard every 30 seconds
    let interval: ReturnType<typeof setInterval>;
    if (autoRefresh) {
      interval = setInterval(() => {
        fetchDashboardData(signal);
        fetchHistoricalData(signal);
      }, 30000);
    }

    return () => {
      signal.aborted = true;
      if (interval) clearInterval(interval);
    };
  }, [autoRefresh, fetchDashboardData, fetchHistoricalData]);

  if (loading) {
    return <div className={styles.loading}>Loading dashboard...</div>;
  }

  if (error) {
    return <div className={styles.error}>Error loading dashboard: {error}</div>;
  }

  if (!dashboardData) {
    return <div className={styles.error}>No dashboard data available</div>;
  }

  return (
    <div className={styles.dashboard}>
      {showConnectorWizard && (
        <ConnectorWizard onClose={() => setShowConnectorWizard(false)} />
      )}

      {/* "Connect your world" CTA — shown when user has no KN connectors yet */}
      {hasConnectors === false && (
        <div style={{
          display: 'flex', alignItems: 'center', gap: 12,
          padding: '12px 16px', marginBottom: 16,
          background: 'rgba(137, 180, 250, 0.08)',
          border: '1px solid rgba(137, 180, 250, 0.25)',
          borderRadius: 8,
        }}>
          <span style={{ fontSize: 22 }}>🔗</span>
          <div style={{ flex: 1 }}>
            <strong style={{ fontSize: 13, color: 'var(--text-primary, #cdd6f4)' }}>Connect your world</strong>
            <p style={{ margin: '2px 0 0', fontSize: 12, color: 'var(--text-secondary, #7f849c)' }}>
              Link Gmail, Google Calendar, Outlook, and more so NexiBot can search your email and meetings.
            </p>
          </div>
          <button
            onClick={() => setShowConnectorWizard(true)}
            style={{
              padding: '6px 14px', background: 'var(--primary)', color: 'var(--background)',
              border: 'none', borderRadius: 6, fontSize: 12, fontWeight: 500, cursor: 'pointer',
            }}
            data-testid="dashboard-connect-btn"
          >
            Get started
          </button>
        </div>
      )}

      <div className={styles.header}>
        <h1>System Monitoring Dashboard</h1>
        <div className={styles.controls}>
          <label>
            <input
              type="checkbox"
              checked={autoRefresh}
              onChange={(e) => setAutoRefresh(e.target.checked)}
            />
            Auto-refresh (30s)
          </label>
          <button onClick={() => { fetchDashboardData(); fetchHistoricalData(); }}>
            Refresh Now
          </button>
        </div>
      </div>

      {/* Top row: System metrics */}
      <div className={styles.section}>
        <h2>System Resources</h2>
        <SystemMetrics data={dashboardData.system_metrics} />
      </div>

      {/* Service health */}
      <div className={styles.section}>
        <h2>Service Health</h2>
        <ServiceHealth services={dashboardData.services} />
      </div>

      {/* Voice, Session, API metrics in a grid */}
      <div className={styles.metricsGrid}>
        <div className={styles.section}>
          <h2>Voice Metrics</h2>
          <VoiceMetrics data={dashboardData.voice_metrics} />
        </div>
        <div className={styles.section}>
          <h2>Session Metrics</h2>
          <SessionMetrics data={dashboardData.session_metrics} />
        </div>
        <div className={styles.section}>
          <h2>API Metrics</h2>
          <ApiMetrics data={dashboardData.api_metrics} />
        </div>
      </div>

      {/* Historical data charts */}
      {historicalData && (
        <div className={styles.section}>
          <h2>Historical Trends</h2>
          <div className={styles.chartsGrid}>
            <HistoricalChart
              title="CPU Usage (%)"
              data={historicalData.cpu_history}
              yAxisMax={100}
              color="var(--error)"
            />
            <HistoricalChart
              title="Memory Usage (%)"
              data={historicalData.memory_history}
              yAxisMax={100}
              color="var(--success)"
            />
            <HistoricalChart
              title="Message Throughput"
              data={historicalData.message_throughput_history}
              color="var(--info)"
            />
            <HistoricalChart
              title="API Latency (ms)"
              data={historicalData.api_latency_history}
              color="var(--warning)"
            />
            <HistoricalChart
              title="Error Rate (%)"
              data={historicalData.error_rate_history}
              yAxisMax={100}
              color="var(--error)"
            />
          </div>
        </div>
      )}

      {/* Alerts */}
      {dashboardData.alerts.length > 0 && (
        <div className={styles.section}>
          <h2>Alerts ({dashboardData.alerts.length})</h2>
          <AlertsPanel alerts={dashboardData.alerts} />
        </div>
      )}

      {/* Database info */}
      {dashboardData.databases.length > 0 && (
        <div className={styles.section}>
          <h2>Databases</h2>
          <div className={styles.databasesTable}>
            <table>
              <thead>
                <tr>
                  <th>Name</th>
                  <th>Size</th>
                  <th>Tables</th>
                  <th>Rows</th>
                  <th>Last Backup</th>
                  <th>Health</th>
                  <th>Fragmentation</th>
                </tr>
              </thead>
              <tbody>
                {dashboardData.databases.map((db, idx) => (
                  <tr key={idx}>
                    <td>{db.name}</td>
                    <td>{(db.size_bytes / 1024 / 1024).toFixed(2)} MB</td>
                    <td>{db.table_count}</td>
                    <td>{db.row_count.toLocaleString()}</td>
                    <td>{db.last_backup ? new Date(db.last_backup).toLocaleDateString() : 'Never'}</td>
                    <td className={db.is_healthy ? styles.healthy : styles.unhealthy}>
                      {db.is_healthy ? 'Healthy' : 'Unhealthy'}
                    </td>
                    <td>{db.fragmentation.toFixed(1)}%</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}

      <div className={styles.footer}>
        <p>Last updated: {new Date(dashboardData.timestamp).toLocaleTimeString()}</p>
      </div>
    </div>
  );
}
