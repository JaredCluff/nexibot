import React from 'react';
import styles from './MetricsCard.module.css';

interface SessionMetricsProps {
  data: {
    active_sessions: number;
    sessions_today: number;
    messages_today: number;
    avg_message_latency_ms: number;
    messages_per_minute: number;
    error_rate: number;
  };
}

export default function SessionMetrics({ data }: SessionMetricsProps) {
  return (
    <div className={styles.metricsCard}>
      <div className={styles.row}>
        <div className={styles.metric}>
          <div className={styles.label}>Active Sessions</div>
          <div className={styles.value}>{data.active_sessions}</div>
        </div>
        <div className={styles.metric}>
          <div className={styles.label}>Sessions Today</div>
          <div className={styles.value}>{data.sessions_today}</div>
        </div>
      </div>
      <div className={styles.row}>
        <div className={styles.metric}>
          <div className={styles.label}>Messages Today</div>
          <div className={styles.value}>{data.messages_today.toLocaleString()}</div>
        </div>
        <div className={styles.metric}>
          <div className={styles.label}>Avg Message Latency</div>
          <div className={styles.value}>{data.avg_message_latency_ms}ms</div>
        </div>
      </div>
      <div className={styles.row}>
        <div className={styles.metric}>
          <div className={styles.label}>Throughput (msg/min)</div>
          <div className={styles.value}>{data.messages_per_minute.toFixed(1)}</div>
        </div>
        <div className={styles.metric}>
          <div className={styles.label}>Error Rate</div>
          <div className={styles.value} style={{
            color: data.error_rate > 5 ? 'var(--error)' : 'var(--success)'
          }}>
            {data.error_rate.toFixed(2)}%
          </div>
        </div>
      </div>
    </div>
  );
}
