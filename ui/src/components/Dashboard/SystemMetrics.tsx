import React from 'react';
import styles from './SystemMetrics.module.css';

interface SystemMetricsProps {
  data: {
    cpu_usage: number;
    memory_used: number;
    memory_total: number;
    memory_used_percent: number;
    disk_used: number;
    disk_total: number;
    disk_used_percent: number;
  };
}

export default function SystemMetrics({ data }: SystemMetricsProps) {
  const formatBytes = (bytes: number) => {
    const gb = bytes / 1024 / 1024 / 1024;
    return gb.toFixed(2) + ' GB';
  };

  const getCPUColor = (usage: number) => {
    if (usage < 50) return 'var(--success)';
    if (usage < 80) return 'var(--warning)';
    return 'var(--error)';
  };

  const getMemoryColor = (percent: number) => {
    if (percent < 50) return 'var(--success)';
    if (percent < 80) return 'var(--warning)';
    return 'var(--error)';
  };

  return (
    <div className={styles.metrics}>
      <div className={styles.metric}>
        <div className={styles.metricTitle}>CPU Usage</div>
        <div className={styles.metricValue}>
          <svg className={styles.gauge} viewBox="0 0 100 100">
            <circle cx="50" cy="50" r="45" className={styles.gaugeBackground} />
            <circle
              cx="50"
              cy="50"
              r="45"
              className={styles.gaugeFill}
              style={{
                strokeDasharray: `${data.cpu_usage * 0.9} 283`,
                stroke: getCPUColor(data.cpu_usage),
              }}
            />
            <text x="50" y="55" textAnchor="middle" className={styles.gaugeText}>
              {data.cpu_usage.toFixed(1)}%
            </text>
          </svg>
        </div>
      </div>

      <div className={styles.metric}>
        <div className={styles.metricTitle}>Memory Usage</div>
        <div className={styles.metricValue}>
          <svg className={styles.gauge} viewBox="0 0 100 100">
            <circle cx="50" cy="50" r="45" className={styles.gaugeBackground} />
            <circle
              cx="50"
              cy="50"
              r="45"
              className={styles.gaugeFill}
              style={{
                strokeDasharray: `${data.memory_used_percent * 0.9} 283`,
                stroke: getMemoryColor(data.memory_used_percent),
              }}
            />
            <text x="50" y="50" textAnchor="middle" className={styles.gaugeText} fontSize="10">
              {data.memory_used_percent.toFixed(1)}%
            </text>
            <text x="50" y="65" textAnchor="middle" className={styles.gaugeSubtext} fontSize="8">
              {formatBytes(data.memory_used)} / {formatBytes(data.memory_total)}
            </text>
          </svg>
        </div>
      </div>

      <div className={styles.metric}>
        <div className={styles.metricTitle}>Disk Usage</div>
        <div className={styles.metricValue}>
          <div className={styles.barChart}>
            <div className={styles.barContainer}>
              <div
                className={styles.bar}
                style={{
                  width: `${data.disk_used_percent}%`,
                  backgroundColor: getMemoryColor(data.disk_used_percent),
                }}
              />
            </div>
            <div className={styles.barLabel}>
              {data.disk_used_percent.toFixed(1)}% ({formatBytes(data.disk_used)} / {formatBytes(data.disk_total)})
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
