import React, { useMemo } from 'react';
import styles from './HistoricalChart.module.css';

interface DataPoint {
  timestamp: string;
  value: number;
}

interface HistoricalChartProps {
  title: string;
  data: DataPoint[];
  yAxisMax?: number;
  color?: string;
}

export default function HistoricalChart({
  title,
  data,
  yAxisMax = undefined,
  color = 'var(--success)',
}: HistoricalChartProps) {
  const { maxValue, minValue, points } = useMemo(() => {
    if (data.length === 0) {
      return { maxValue: 100, minValue: 0, points: [] };
    }

    const values = data.map((d) => d.value);
    const max = Math.max(...values, yAxisMax || 0);
    const min = Math.min(...values, 0);

    const width = 400;
    const height = 150;
    const padding = 20;

    const points = data.map((d, idx) => {
      const x = (padding + ((idx / (data.length - 1 || 1)) * (width - padding * 2)));
      const y = (height - padding - ((d.value - min) / (max - min || 1)) * (height - padding * 2));
      return { x, y, value: d.value };
    });

    return { maxValue: max, minValue: min, points };
  }, [data, yAxisMax]);

  if (data.length === 0) {
    return (
      <div className={styles.chart}>
        <div className={styles.title}>{title}</div>
        <div className={styles.empty}>No data</div>
      </div>
    );
  }

  const pathData = points
    .map((p, i) => `${i === 0 ? 'M' : 'L'} ${p.x} ${p.y}`)
    .join(' ');

  return (
    <div className={styles.chart}>
      <div className={styles.title}>{title}</div>
      <svg className={styles.chartSvg} viewBox="0 0 420 170">
        {/* Grid lines */}
        {[0, 0.25, 0.5, 0.75, 1].map((fraction, idx) => (
          <line
            key={idx}
            x1="20"
            y1={20 + fraction * 130}
            x2="400"
            y2={20 + fraction * 130}
            className={styles.gridLine}
          />
        ))}

        {/* Y-axis label */}
        <text x="10" y="30" className={styles.axisLabel}>
          {maxValue.toFixed(0)}
        </text>
        <text x="10" y="150" className={styles.axisLabel}>
          {minValue.toFixed(0)}
        </text>

        {/* Line chart */}
        <path d={pathData} className={styles.line} style={{ stroke: color }} />

        {/* Area under curve */}
        <path
          d={`${pathData} L ${points[points.length - 1].x} 150 L 20 150 Z`}
          className={styles.area}
          style={{ fill: color, opacity: 0.2 }}
        />

        {/* Data points */}
        {points.map((p, idx) => (
          <circle key={idx} cx={p.x} cy={p.y} r="2" className={styles.dot} style={{ fill: color }} />
        ))}

        {/* X and Y axes */}
        <line x1="20" y1="20" x2="20" y2="150" className={styles.axis} />
        <line x1="20" y1="150" x2="400" y2="150" className={styles.axis} />
      </svg>
      <div className={styles.legend}>
        <span>Last 24 hours</span>
        <span style={{ color }}>Current: {data[data.length - 1]?.value.toFixed(1) || 'N/A'}</span>
      </div>
    </div>
  );
}
