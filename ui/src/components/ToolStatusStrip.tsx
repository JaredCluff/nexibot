/**
 * ToolStatusStrip — per-tool status strip shown during agentic tool loops.
 *
 * Displays a spinner/check/✗ icon, the tool display name, and on error a
 * plain-English reason with an optional countdown before the next retry.
 */
import { useEffect, useRef, useState } from 'react';
import type { ToolIndicator } from './chat-types';

// ─── Tool name → user-friendly label ─────────────────────────────────────────

const TOOL_LABELS: Record<string, string> = {
  nexibot_execute: 'Run Command',
  nexibot_fetch: 'Fetch URL',
  nexibot_filesystem: 'File System',
  nexibot_memory: 'Memory',
  nexibot_search: 'Search',
  nexibot_k2k_search: 'K2K Search',
  nexibot_memory_search: 'Memory Search',
  nexibot_soul: 'Soul',
  nexibot_settings: 'Settings',
  nexibot_browser: 'Browser',
  nexibot_background_task: 'Background Task',
  computer_use: 'Computer Use',
  kb_search: 'Knowledge Base',
  kb_read: 'Knowledge Base',
  get_emails: 'Email',
  list_emails: 'Email',
  get_calendar_events: 'Calendar',
  list_calendar_events: 'Calendar',
  get_contacts: 'Contacts',
  mcp_read: 'MCP Read',
  mcp_list: 'MCP List',
};

function toolLabel(name: string): string {
  return TOOL_LABELS[name] ?? name.replace(/_/g, ' ').replace(/\b\w/g, (c) => c.toUpperCase());
}

// ─── Countdown hook ───────────────────────────────────────────────────────────

function useCountdown(initialSecs: number | undefined): number {
  const [remaining, setRemaining] = useState(initialSecs ?? 0);
  const ref = useRef(initialSecs ?? 0);

  useEffect(() => {
    if (!initialSecs) return;
    ref.current = initialSecs;
    setRemaining(initialSecs);
    const id = setInterval(() => {
      ref.current = Math.max(0, ref.current - 1);
      setRemaining(ref.current);
      if (ref.current <= 0) clearInterval(id);
    }, 1000);
    return () => clearInterval(id);
  }, [initialSecs]);

  return remaining;
}

// ─── Single tool row ──────────────────────────────────────────────────────────

function ToolRow({ tool }: { tool: ToolIndicator }) {
  const countdown = useCountdown(tool.retryCountdown);

  const iconEl = (() => {
    switch (tool.status) {
      case 'running':
        return <span className="tool-strip-spinner" aria-label="running" />;
      case 'retrying':
        return <span className="tool-strip-spinner tool-strip-spinner--warn" aria-label="retrying" />;
      case 'done':
        return <span className="tool-strip-icon tool-strip-icon--ok" aria-label="done">✓</span>;
      case 'error':
        return <span className="tool-strip-icon tool-strip-icon--error" aria-label="failed">✗</span>;
    }
  })();

  return (
    <div className={`tool-strip-row tool-strip-row--${tool.status}`}>
      {iconEl}
      <span className="tool-strip-name">{toolLabel(tool.name)}</span>
      {tool.status === 'retrying' && (
        <span className="tool-strip-retry">
          {countdown > 0 ? `Retrying in ${countdown}s…` : 'Retrying…'}
          {tool.attempt != null && tool.maxAttempts != null && (
            <span className="tool-strip-attempt"> ({tool.attempt}/{tool.maxAttempts})</span>
          )}
        </span>
      )}
      {tool.status === 'error' && tool.errorMessage && (
        <span className="tool-strip-error-msg" title={tool.errorMessage}>
          {tool.errorMessage.length > 80 ? tool.errorMessage.slice(0, 80) + '…' : tool.errorMessage}
        </span>
      )}
    </div>
  );
}

// ─── Strip component ──────────────────────────────────────────────────────────

interface ToolStatusStripProps {
  tools: ToolIndicator[];
}

export default function ToolStatusStrip({ tools }: ToolStatusStripProps) {
  if (tools.length === 0) return null;

  return (
    <div className="tool-status-strip" role="status" aria-live="polite" aria-label="Tool execution status">
      {tools.map((t) => (
        <ToolRow key={t.id} tool={t} />
      ))}
    </div>
  );
}
