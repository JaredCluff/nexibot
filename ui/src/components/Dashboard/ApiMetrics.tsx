import React from 'react';
import styles from './MetricsCard.module.css';

interface ApiMetricsProps {
  data: {
    total_requests: number;
    successful_requests: number;
    failed_requests: number;
    p99_latency_ms: number;
    slowest_endpoint: string;
    most_requested_endpoint: string;
    cache_hit_rate: number;
  };
}

export default function ApiMetrics({ data }: ApiMetricsProps) {
  const successRate = data.total_requests > 0
    ? (data.successful_requests / data.total_requests * 100)
    : 0;

  return (
    <div className={styles.metricsCard}>
      <div className={styles.row}>
        <div className={styles.metric}>
          <div className={styles.label}>Total Requests</div>
          <div className={styles.value}>{data.total_requests.toLocaleString()}</div>
        </div>
        <div className={styles.metric}>
          <div className={styles.label}>Success Rate</div>
          <div className={styles.value} style={{
            color: successRate > 95 ? 'var(--success)' : 'var(--error)'
          }}>
            {successRate.toFixed(1)}%
          </div>
        </div>
      </div>
      <div className={styles.row}>
        <div className={styles.metric}>
          <div className={styles.label}>P99 Latency</div>
          <div className={styles.value}>{data.p99_latency_ms}ms</div>
        </div>
        <div className={styles.metric}>
          <div className={styles.label}>Cache Hit Rate</div>
          <div className={styles.value}>{data.cache_hit_rate.toFixed(1)}%</div>
        </div>
      </div>
      <div className={styles.row}>
        <div className={styles.metric}>
          <div className={styles.label}>Most Requested</div>
          <div className={styles.value} style={{ fontSize: '0.9em' }}>{data.most_requested_endpoint}</div>
        </div>
        <div className={styles.metric}>
          <div className={styles.label}>Slowest Endpoint</div>
          <div className={styles.value} style={{ fontSize: '0.9em' }}>{data.slowest_endpoint}</div>
        </div>
      </div>
    </div>
  );
}
