import React from 'react';
import styles from './MetricsCard.module.css';

interface VoiceMetricsProps {
  data: {
    wakeword_detections: number;
    successful_transcriptions: number;
    failed_transcriptions: number;
    tts_generations: number;
    avg_stt_latency_ms: number;
    avg_tts_latency_ms: number;
    service_status: string;
  };
}

export default function VoiceMetrics({ data }: VoiceMetricsProps) {
  const successRate = data.successful_transcriptions + data.failed_transcriptions > 0
    ? (data.successful_transcriptions / (data.successful_transcriptions + data.failed_transcriptions) * 100)
    : 0;

  return (
    <div className={styles.metricsCard}>
      <div className={styles.row}>
        <div className={styles.metric}>
          <div className={styles.label}>Wake Word Detections</div>
          <div className={styles.value}>{data.wakeword_detections}</div>
        </div>
        <div className={styles.metric}>
          <div className={styles.label}>STT Success Rate</div>
          <div className={styles.value}>{successRate.toFixed(1)}%</div>
        </div>
      </div>
      <div className={styles.row}>
        <div className={styles.metric}>
          <div className={styles.label}>Avg STT Latency</div>
          <div className={styles.value}>{data.avg_stt_latency_ms}ms</div>
        </div>
        <div className={styles.metric}>
          <div className={styles.label}>Avg TTS Latency</div>
          <div className={styles.value}>{data.avg_tts_latency_ms}ms</div>
        </div>
      </div>
      <div className={styles.row}>
        <div className={styles.metric}>
          <div className={styles.label}>TTS Generations</div>
          <div className={styles.value}>{data.tts_generations}</div>
        </div>
        <div className={styles.metric}>
          <div className={styles.label}>Service Status</div>
          <div className={styles.value} style={{
            color: data.service_status === 'Healthy' ? 'var(--success)' : 'var(--error)'
          }}>
            {data.service_status}
          </div>
        </div>
      </div>
    </div>
  );
}
