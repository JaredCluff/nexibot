import { useState, useEffect, useCallback } from 'react';
import { listen, UnlistenFn } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import ShellViewerToolbar from './ShellViewerToolbar';
import ShellSessionTab, { ShellLogEntry } from './ShellSessionTab';
import { FilterEvent } from './FilterEventPanel';
import { notifyError, notifyInfo, notifyWarn } from '../shared/notify';
import './ShellViewer.css';

interface GatedShellStatus {
  enabled: boolean;
  debug_mode: boolean;
  record_sessions: boolean;
  secret_count: number;
  active_sessions: number;
}

interface SessionState {
  sessionId: string;
  agentId: string;
  entries: ShellLogEntry[];
}

export default function ShellViewerApp() {
  const [status, setStatus] = useState<GatedShellStatus | null>(null);
  const [sessions, setSessions] = useState<Map<string, SessionState>>(new Map());
  const [activeTab, setActiveTab] = useState<string | null>(null);

  // Load initial status
  useEffect(() => {
    invoke<GatedShellStatus>('get_gated_shell_status')
      .then(setStatus)
      .catch((e) => notifyError('NexiGate', `Failed to load shell status: ${e}`));
  }, []);

  // Add entry to a session
  const addEntry = useCallback((sessionId: string, agentId: string, entry: ShellLogEntry) => {
    setSessions(prev => {
      const next = new Map(prev);
      const existing = next.get(sessionId) ?? { sessionId, agentId, entries: [] };
      next.set(sessionId, {
        ...existing,
        entries: [...existing.entries, entry],
      });
      return next;
    });
    // Auto-select if first session
    setActiveTab(prev => prev ?? sessionId);
  }, []);

  // Subscribe to shell:* events
  useEffect(() => {
    const unlisteners: Promise<UnlistenFn>[] = [];

    unlisteners.push(
      listen<{ session_id: string; agent_id: string; timestamp_ms: number }>(
        'shell:session-created',
        ({ payload }) => {
          addEntry(payload.session_id, payload.agent_id, {
            type: 'session',
            timestamp_ms: payload.timestamp_ms,
            text: `Session started for ${payload.agent_id}`,
          });
          setActiveTab(prev => prev ?? payload.session_id);
        }
      )
    );

    unlisteners.push(
      listen<{ session_id: string }>('shell:session-closed', ({ payload }) => {
        setSessions(prev => {
          const next = new Map(prev);
          const s = next.get(payload.session_id);
          if (s) {
            next.set(payload.session_id, {
              ...s,
              entries: [
                ...s.entries,
                {
                  type: 'session',
                  timestamp_ms: Date.now(),
                  text: 'Session closed',
                },
              ],
            });
          }
          return next;
        });
      })
    );

    unlisteners.push(
      listen<{
        session_id: string;
        agent_id: string;
        filtered_command: string;
        timestamp_ms: number;
      }>('shell:command', ({ payload }) => {
        addEntry(payload.session_id, payload.agent_id, {
          type: 'command',
          timestamp_ms: payload.timestamp_ms,
          text: payload.filtered_command,
        });
      })
    );

    unlisteners.push(
      listen<{
        session_id: string;
        agent_id: string;
        filtered_output: string;
        exit_code: number;
        duration_ms: number;
      }>('shell:output', ({ payload }) => {
        addEntry(payload.session_id, payload.agent_id, {
          type: 'output',
          timestamp_ms: Date.now(),
          text: payload.filtered_output,
          exit_code: payload.exit_code,
          duration_ms: payload.duration_ms,
        });
      })
    );

    unlisteners.push(
      listen<{
        session_id: string;
        filter_events: FilterEvent[];
      }>('shell:filter-event', ({ payload }) => {
        const count = payload.filter_events.length;
        setSessions(prev => {
          const next = new Map(prev);
          const s = next.get(payload.session_id);
          if (s) {
            // Attach filter events to the last output entry
            const entries = [...s.entries];
            const lastOutput = entries.findLastIndex(e => e.type === 'output');
            if (lastOutput >= 0) {
              entries[lastOutput] = {
                ...entries[lastOutput],
                filter_events: payload.filter_events,
              };
            } else {
              entries.push({
                type: 'filter',
                timestamp_ms: Date.now(),
                text: `${count} secret${count !== 1 ? 's' : ''} masked`,
                filter_events: payload.filter_events,
              });
            }
            next.set(payload.session_id, { ...s, entries });
          }
          return next;
        });
      })
    );

    unlisteners.push(
      listen<{
        session_id: string;
        command_preview: string;
        reason: string;
      }>('shell:policy-deny', ({ payload }) => {
        setSessions(prev => {
          const next = new Map(prev);
          const s = next.get(payload.session_id) ?? {
            sessionId: payload.session_id,
            agentId: 'unknown',
            entries: [],
          };
          next.set(payload.session_id, {
            ...s,
            entries: [
              ...s.entries,
              {
                type: 'deny',
                timestamp_ms: Date.now(),
                text: `DENIED: ${payload.command_preview}  [${payload.reason}]`,
              },
            ],
          });
          return next;
        });
      })
    );

    unlisteners.push(
      listen<{ enabled: boolean; debug_mode: boolean }>('shell:status-changed', ({ payload }) => {
        setStatus(prev =>
          prev
            ? { ...prev, enabled: payload.enabled, debug_mode: payload.debug_mode }
            : null
        );
      })
    );

    // Part 2: Dynamic Discovery + Plugin events
    unlisteners.push(
      listen<{
        session_id: string;
        proxy_token: string;
        format: string;
        source: string;
      }>('shell:secret-discovered', ({ payload }) => {
        addEntry(payload.session_id, '', {
          type: 'secret',
          timestamp_ms: Date.now(),
          text: `Secret auto-discovered (${payload.format}) via ${payload.source} → masked as ${payload.proxy_token}`,
        });
      })
    );

    unlisteners.push(
      listen<{
        plugin_name: string;
        event_type: string;
        decision_type: string;
        reason?: string;
      }>('shell:plugin-decision', ({ payload }) => {
        if (payload.decision_type === 'Deny') {
          notifyWarn('Plugin', `${payload.plugin_name} vetoed ${payload.event_type}${payload.reason ? `: ${payload.reason}` : ''}`);
        } else if (payload.decision_type === 'RegisterSecret') {
          notifyInfo('Plugin', `${payload.plugin_name} registered a new secret`);
        }
      })
    );

    unlisteners.push(
      listen<{ name: string; version: string; author: string }>(
        'shell:plugin-loaded',
        ({ payload }) => {
          notifyInfo('Plugin', `Loaded: ${payload.name} v${payload.version} by ${payload.author}`);
        }
      )
    );

    unlisteners.push(
      listen<{ name: string; error: string }>(
        'shell:plugin-error',
        ({ payload }) => {
          notifyError('Plugin', `${payload.name}: ${payload.error}`);
        }
      )
    );

    return () => {
      unlisteners.forEach(p => p.then(fn => fn()));
    };
  }, [addEntry]);

  const closeTab = useCallback(
    async (sessionId: string) => {
      try {
        await invoke('close_shell_session', { sessionKey: sessionId });
      } catch (e) {
        notifyError('NexiGate', `Failed to close session: ${e}`);
      }
      setSessions(prev => {
        const next = new Map(prev);
        next.delete(sessionId);
        return next;
      });
      if (activeTab === sessionId) {
        const remaining = Array.from(sessions.keys()).filter(k => k !== sessionId);
        setActiveTab(remaining[0] ?? null);
      }
    },
    [activeTab, sessions]
  );

  const sessionList = Array.from(sessions.values());
  const activeSession = activeTab ? sessions.get(activeTab) : null;

  return (
    <div className="shell-viewer">
      <ShellViewerToolbar status={status} onStatusChange={setStatus} />

      {/* Session tabs */}
      <div className="shell-tabs">
        {sessionList.map(s => (
          <div
            key={s.sessionId}
            className={`shell-tab ${activeTab === s.sessionId ? 'active' : ''}`}
            onClick={() => setActiveTab(s.sessionId)}
          >
            <span>{s.agentId}</span>
            <button
              className="shell-tab-close"
              onClick={e => {
                e.stopPropagation();
                closeTab(s.sessionId);
              }}
              title="Close session"
            >
              ×
            </button>
          </div>
        ))}
      </div>

      {/* Active session log */}
      {activeSession ? (
        <ShellSessionTab
          sessionId={activeSession.sessionId}
          entries={activeSession.entries}
        />
      ) : (
        <div className="shell-empty-state" style={{ flex: 1 }}>
          <h3>NexiGate Shell Viewer</h3>
          <p>
            {status?.enabled
              ? 'No sessions yet. Shell commands will appear here when executed through the gate.'
              : 'The gated shell is currently disabled. Enable it in the toolbar above or in Settings → NexiGate.'}
          </p>
          {!status?.enabled && (
            <div className="shell-gate-disabled">Gate is OFF — commands bypass NexiGate</div>
          )}
        </div>
      )}
    </div>
  );
}
