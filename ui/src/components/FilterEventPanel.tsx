import { useState } from 'react';

export interface FilterEvent {
  direction: 'inbound' | 'outbound';
  proxy_key: string;
  key_format: string;
  position: number;
  length: number;
  timestamp_ms: number;
}

interface Props {
  events: FilterEvent[];
}

export default function FilterEventPanel({ events }: Props) {
  const [expanded, setExpanded] = useState(false);

  if (events.length === 0) return null;

  return (
    <div className="filter-event-panel">
      <div className="filter-event-toggle" onClick={() => setExpanded(!expanded)}>
        <span>{expanded ? '▼' : '▶'}</span>
        <span>
          🔑 {events.length} filter event{events.length !== 1 ? 's' : ''}
        </span>
      </div>

      {expanded && (
        <div className="filter-event-content">
          {events.map((ev, i) => (
            <div key={i} className="filter-event-row">
              <span className="filter-event-dir">
                {ev.direction === 'inbound' ? '→ PTY' : '← agent'}
              </span>
              <span className="filter-event-proxy" title={ev.proxy_key}>
                {ev.proxy_key.length > 30
                  ? `${ev.proxy_key.slice(0, 30)}…`
                  : ev.proxy_key}
              </span>
              <span className="filter-event-format">[{ev.key_format}]</span>
              <span style={{ color: 'var(--text-secondary)' }}>
                @{ev.position}+{ev.length}
              </span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
