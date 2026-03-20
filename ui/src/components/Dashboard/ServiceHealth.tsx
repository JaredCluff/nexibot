import React from 'react';
import styles from './ServiceHealth.module.css';

interface Service {
  name: string;
  status: string;
  message: string;
  response_time_ms: number;
  last_check: string;
  failure_count: number;
  uptime: number;
}

interface ServiceHealthProps {
  services: Service[];
}

export default function ServiceHealth({ services }: ServiceHealthProps) {
  const getStatusColor = (status: string) => {
    switch (status) {
      case 'Healthy':
        return 'var(--success)';
      case 'Degraded':
        return 'var(--warning)';
      case 'Unhealthy':
        return 'var(--error)';
      case 'Offline':
        return 'var(--text-secondary)';
      default:
        return 'var(--text-secondary)';
    }
  };

  const getStatusIcon = (status: string) => {
    switch (status) {
      case 'Healthy':
        return '✓';
      case 'Degraded':
        return '⚠';
      case 'Unhealthy':
      case 'Offline':
        return '✕';
      default:
        return '?';
    }
  };

  if (services.length === 0) {
    return <div className={styles.empty}>No services monitored</div>;
  }

  return (
    <div className={styles.services}>
      {services.map((service, idx) => (
        <div key={idx} className={styles.serviceCard}>
          <div className={styles.serviceHeader}>
            <div
              className={styles.statusIndicator}
              style={{ backgroundColor: getStatusColor(service.status) }}
              title={service.status}
            >
              {getStatusIcon(service.status)}
            </div>
            <div className={styles.serviceName}>{service.name}</div>
            <div className={styles.uptime}>{service.uptime.toFixed(1)}%</div>
          </div>
          <div className={styles.serviceMessage}>{service.message}</div>
          <div className={styles.serviceDetails}>
            <span>Response: {service.response_time_ms}ms</span>
            <span>Failures: {service.failure_count}</span>
            <span>Last: {new Date(service.last_check).toLocaleTimeString()}</span>
          </div>
        </div>
      ))}
    </div>
  );
}
