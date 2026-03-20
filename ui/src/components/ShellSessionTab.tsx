import { useEffect, useRef } from 'react';
import FilterEventPanel, { FilterEvent } from './FilterEventPanel';

export interface ShellLogEntry {
  type: 'command' | 'output' | 'filter' | 'deny' | 'session' | 'secret' | 'plugin';
  timestamp_ms: number;
  text: string;
  exit_code?: number;
  duration_ms?: number;
  filter_events?: FilterEvent[];
}

interface Props {
  sessionId: string;
  entries: ShellLogEntry[];
}

function formatTime(ms: number): string {
  const d = new Date(ms);
  return d.toLocaleTimeString('en-US', { hour12: false });
}

export default function ShellSessionTab({ sessionId, entries }: Props) {
  const bottomRef = useRef<HTMLDivElement>(null);

  // Auto-scroll to bottom on new entries
  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [entries.length]);

  if (entries.length === 0) {
    return (
      <div className="shell-session-tab">
        <div className="shell-empty-state">
          <h3>No activity yet</h3>
          <p>Commands executed through NexiGate will appear here in real-time.</p>
        </div>
      </div>
    );
  }

  return (
    <div className="shell-session-tab">
      <div className="shell-log">
        {entries.map((entry, i) => (
          <div key={i} className={`shell-log-entry shell-entry-${entry.type}`}>
            <span className="shell-log-time">{formatTime(entry.timestamp_ms)}</span>

            <span className="shell-log-icon">
              {entry.type === 'command' && '▶'}
              {entry.type === 'output' && '◀'}
              {entry.type === 'filter' && '🔑'}
              {entry.type === 'deny' && '🚫'}
              {entry.type === 'session' && '◉'}
              {entry.type === 'secret' && '🔐'}
              {entry.type === 'plugin' && '🔌'}
            </span>

            <span className="shell-log-text">
              {entry.text}
              {entry.exit_code !== undefined && (
                <>
                  <span
                    className={`shell-exit-badge ${entry.exit_code === 0 ? 'shell-exit-ok' : 'shell-exit-err'}`}
                  >
                    exit:{entry.exit_code}
                  </span>
                  {entry.duration_ms !== undefined && (
                    <span className="shell-duration">| {entry.duration_ms}ms</span>
                  )}
                </>
              )}
            </span>
          </div>
        ))}
        <div ref={bottomRef} />
      </div>

      {/* Show filter events for last output entry */}
      {(() => {
        const last = entries.filter(e => e.filter_events && e.filter_events.length > 0).at(-1);
        return last ? (
          <FilterEventPanel events={last.filter_events!} />
        ) : null;
      })()}
    </div>
  );
}
