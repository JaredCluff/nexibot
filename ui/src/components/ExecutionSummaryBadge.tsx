import { useState } from 'react';
import type { ExecutionSummary } from './chat-types';

interface Props {
  summary: ExecutionSummary;
}

function countTools(tools: string[]): [string, number][] {
  const counts = new Map<string, number>();
  for (const t of tools) {
    counts.set(t, (counts.get(t) ?? 0) + 1);
  }
  return Array.from(counts.entries());
}

export default function ExecutionSummaryBadge({ summary }: Props) {
  const [expanded, setExpanded] = useState(false);
  const elapsedSec = (summary.elapsed_ms / 1000).toFixed(1);
  const toolCounts = countTools(summary.tools_called);

  return (
    <div className="execution-summary">
      <button
        className="execution-summary-badge"
        onClick={() => setExpanded(e => !e)}
        aria-expanded={expanded}
        title={expanded ? 'Hide execution details' : 'Show execution details'}
      >
        <span className="exec-icon">&#9881;</span>
        <span className="exec-steps">{summary.iterations_used} step{summary.iterations_used !== 1 ? 's' : ''}</span>
        <span className="exec-sep">&middot;</span>
        <span className="exec-time">{elapsedSec}s</span>
        {summary.tools_called.length > 0 && (
          <>
            <span className="exec-sep">&middot;</span>
            <span className="exec-tool-count">{summary.tools_called.length} tool call{summary.tools_called.length !== 1 ? 's' : ''}</span>
          </>
        )}
        <span className="exec-chevron">{expanded ? '\u25b4' : '\u25be'}</span>
      </button>

      {expanded && (
        <div className="execution-summary-detail">
          {toolCounts.length > 0 && (
            <ul className="exec-tool-list">
              {toolCounts.map(([name, count]) => (
                <li key={name} className="exec-tool-item">
                  {count > 1
                    ? <code>{name} (×{count})</code>
                    : <code>{name}</code>
                  }
                </li>
              ))}
            </ul>
          )}
          {summary.fallbacks.length > 0 && (
            <div className="exec-fallbacks">
              {summary.fallbacks.map(([from, to, reason], i) => (
                <div key={i} className="exec-fallback-item">
                  Fallback: <code>{from}</code> &rarr; <code>{to}</code> ({reason})
                </div>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
