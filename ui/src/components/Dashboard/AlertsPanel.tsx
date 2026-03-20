import React from 'react';
import styles from './AlertsPanel.module.css';

interface Alert {
  severity: string;
  message: string;
  service: string;
  timestamp: string;
}

interface AlertsPanelProps {
  alerts: Alert[];
}

export default function AlertsPanel({ alerts }: AlertsPanelProps) {
  const getSeverityColor = (severity: string) => {
    switch (severity) {
      case 'info':
        return 'var(--info)';
      case 'warning':
        return 'var(--warning)';
      case 'error':
        return 'var(--error)';
      default:
        return 'var(--text-secondary)';
    }
  };

  const getSeverityIcon = (severity: string) => {
    switch (severity) {
      case 'info':
        return 'ℹ';
      case 'warning':
        return '⚠';
      case 'error':
        return '✕';
      default:
        return '•';
    }
  };

  return (
    <div className={styles.alertsPanel}>
      {alerts.map((alert, idx) => (
        <div key={idx} className={styles.alertItem}>
          <div
            className={styles.severityBadge}
            style={{ backgroundColor: getSeverityColor(alert.severity) }}
          >
            {getSeverityIcon(alert.severity)}
          </div>
          <div className={styles.alertContent}>
            <div className={styles.alertMessage}>{alert.message}</div>
            <div className={styles.alertMeta}>
              <span className={styles.service}>{alert.service}</span>
              <span className={styles.timestamp}>{new Date(alert.timestamp).toLocaleTimeString()}</span>
            </div>
          </div>
        </div>
      ))}
    </div>
  );
}
