import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useSettings } from '../SettingsContext';
import { InfoTip } from '../shared/InfoTip';
import { notifyError } from '../../../shared/notify';
import { useConfirm } from '../../../shared/useConfirm';

interface NamedSession {
  id: string;
  name: string;
  created_at: string;
  message_count: number;
  channel_type: string;
}

interface InterSessionMessage {
  from_session: string;
  to_session: string;
  content: string;
  timestamp: string;
}

export function AgentsTab() {
  const { agents, activeGuiAgent, setActiveGuiAgent } = useSettings();
  const { confirm: showConfirm, modal: confirmModal } = useConfirm();

  // Named Sessions state
  const [sessions, setSessions] = useState<NamedSession[]>([]);
  const [showCreateSession, setShowCreateSession] = useState(false);
  const [newSessionName, setNewSessionName] = useState('');
  const [expandedInbox, setExpandedInbox] = useState<string | null>(null);
  const [inboxMessages, setInboxMessages] = useState<InterSessionMessage[]>([]);

  // Inter-Agent Messaging state
  const [msgFrom, setMsgFrom] = useState('');
  const [msgTo, setMsgTo] = useState('');
  const [msgContent, setMsgContent] = useState('');
  const [sendingMsg, setSendingMsg] = useState(false);

  const loadSessions = async () => {
    try {
      const list = await invoke<NamedSession[]>('list_named_sessions');
      setSessions(list);
    } catch { /* not critical */ }
  };

  useEffect(() => {
    loadSessions();
  }, []);

  const handleCreateSession = async () => {
    if (!newSessionName.trim()) return;
    try {
      await invoke('create_named_session', { name: newSessionName.trim() });
      setNewSessionName('');
      setShowCreateSession(false);
      loadSessions();
    } catch (error) {
      notifyError('Sessions', `Failed to create session: ${error}`);
    }
  };

  const handleDeleteSession = async (sessionId: string) => {
    if (!await showConfirm('Delete this named session?', { danger: true })) return;
    try {
      await invoke('delete_named_session', { sessionId });
      if (expandedInbox === sessionId) setExpandedInbox(null);
      loadSessions();
    } catch (error) {
      notifyError('Sessions', `Failed to delete session: ${error}`);
    }
  };

  const handleSwitchSession = async (sessionId: string) => {
    try {
      await invoke('switch_named_session', { sessionId });
    } catch (error) {
      notifyError('Sessions', `Failed to switch: ${error}`);
    }
  };

  const handleViewInbox = async (sessionId: string) => {
    if (expandedInbox === sessionId) {
      setExpandedInbox(null);
      return;
    }
    try {
      const messages = await invoke<InterSessionMessage[]>('get_session_inbox', { sessionId });
      setInboxMessages(messages);
      setExpandedInbox(sessionId);
    } catch (error) {
      notifyError('Sessions', `Failed to load inbox: ${error}`);
    }
  };

  const handleSendMessage = async () => {
    if (!msgFrom || !msgTo || !msgContent.trim()) return;
    setSendingMsg(true);
    try {
      await invoke('send_inter_session_message', {
        fromSession: msgFrom,
        toSession: msgTo,
        content: msgContent.trim(),
      });
      setMsgContent('');
      if (expandedInbox === msgTo) {
        const messages = await invoke<InterSessionMessage[]>('get_session_inbox', { sessionId: msgTo });
        setInboxMessages(messages);
      }
    } catch (error) {
      notifyError('Messaging', `Failed to send message: ${error}`);
    } finally {
      setSendingMsg(false);
    }
  };

  return (
    <div className="tab-content">
      {confirmModal}
      <div className="settings-group">
        <h3>Agents <InfoTip text="Configure multiple AI agents with different models, personalities, and channel bindings. Agents are defined in the config file." /></h3>
        <p className="group-description">
          Agents allow you to run multiple AI personalities, each with their own model, soul, and channel bindings.
          Agents are configured in the YAML config file.
        </p>

        {agents.length === 0 ? (
          <div className="info-text">No agents configured. Using default agent. Add agents in the config file.</div>
        ) : (
          <>
            <label className="field">
              <span>Active GUI Agent <InfoTip text="Which agent handles messages from the desktop chat interface." /></span>
              <select
                value={activeGuiAgent}
                onChange={async (e) => {
                  try {
                    await invoke('set_active_gui_agent', { agentId: e.target.value });
                    setActiveGuiAgent(e.target.value);
                  } catch (error) {
                    notifyError('Agents', `Failed to set active agent: ${error}`);
                  }
                }}
              >
                {agents.map((a) => (
                  <option key={a.id} value={a.id}>{a.name}{a.is_default ? ' (default)' : ''}</option>
                ))}
              </select>
            </label>

            {agents.map((agent) => (
              <div key={agent.id} className="mcp-server-card">
                <div className="mcp-server-header">
                  <span className={`mcp-status-dot`} style={{ backgroundColor: agent.id === activeGuiAgent ? 'var(--success)' : 'var(--text-secondary)' }} />
                  <span className="mcp-server-name">{agent.name}</span>
                  <span className="mcp-server-command">{agent.model || 'default model'}</span>

                  {agent.is_default && <span className="mcp-tool-count">default</span>}
                </div>
                {agent.channel_bindings.length > 0 && (
                  <div style={{ fontSize: '12px', color: 'var(--text-secondary)', margin: '4px 0' }}>
                    Channels: {agent.channel_bindings.map(b => b.peer_id ? `${b.channel}:${b.peer_id}` : b.channel).join(', ')}
                  </div>
                )}
              </div>
            ))}
          </>
        )}
      </div>

      {/* Named Sessions */}
      <div className="settings-group">
        <h3>Named Sessions<InfoTip text="Create isolated conversation sessions with names. Each session has its own inbox for inter-agent messaging." /></h3>
        <p className="group-description">
          Named sessions allow agents to maintain separate conversation contexts and communicate via inboxes.
        </p>

        {sessions.length === 0 && !showCreateSession && (
          <div className="info-text">No named sessions. Create one to get started.</div>
        )}

        {sessions.map((session) => (
          <div key={session.id} className="mcp-server-card">
            <div className="mcp-server-header">
              <span className="mcp-server-name">{session.name}</span>
              <span className="mcp-tool-count">{new Date(session.created_at).toLocaleDateString()}</span>
            </div>
            <div className="action-buttons">
              <button className="primary" onClick={() => handleSwitchSession(session.id)}>Switch To</button>
              <button onClick={() => handleViewInbox(session.id)}>
                {expandedInbox === session.id ? 'Hide Inbox' : 'View Inbox'}
              </button>
              <button className="danger" onClick={() => handleDeleteSession(session.id)}>Delete</button>
            </div>
            {expandedInbox === session.id && (
              <div style={{ marginTop: '8px', borderTop: '1px solid var(--border)', paddingTop: '8px' }}>
                {inboxMessages.length === 0 ? (
                  <div className="info-text">Inbox is empty.</div>
                ) : (
                  inboxMessages.map((msg, i) => (
                    <div key={i} className="inbox-message">
                      <div style={{ fontSize: '11px', color: 'var(--text-secondary)', marginBottom: '2px' }}>
                        From: {msg.from_session} — {new Date(msg.timestamp).toLocaleString()}
                      </div>
                      <div style={{ fontSize: '13px' }}>{msg.content}</div>
                    </div>
                  ))
                )}
              </div>
            )}
          </div>
        ))}

        {showCreateSession ? (
          <div className="mcp-add-form">
            <label className="field">
              <span>Session Name<InfoTip text="A descriptive name for this session." /></span>
              <input
                type="text"
                placeholder="Session name (e.g., Research, Coding)"
                value={newSessionName}
                onChange={(e) => setNewSessionName(e.target.value)}
                onKeyDown={(e) => { if (e.key === 'Enter') handleCreateSession(); }}
              />
            </label>
            <div className="mcp-add-actions">
              <button onClick={handleCreateSession} disabled={!newSessionName.trim()}>Create</button>
              <button onClick={() => setShowCreateSession(false)}>Cancel</button>
            </div>
          </div>
        ) : (
          <button className="mcp-add-btn" onClick={() => setShowCreateSession(true)}>
            + Create Session
          </button>
        )}
      </div>

      {/* Inter-Agent Messaging */}
      {sessions.length >= 2 && (
        <div className="settings-group">
          <h3>Inter-Agent Messaging<InfoTip text="Send messages between named sessions. Useful for agent-to-agent coordination." /></h3>
          <p className="group-description">
            Send messages between sessions for agent coordination and task delegation.
          </p>

          <div className="settings-row">
            <label className="field">
              <span>From Session</span>
              <select value={msgFrom} onChange={(e) => setMsgFrom(e.target.value)}>
                <option value="">Select...</option>
                {sessions.map(s => (
                  <option key={s.id} value={s.id}>{s.name}</option>
                ))}
              </select>
            </label>
            <label className="field">
              <span>To Session</span>
              <select value={msgTo} onChange={(e) => setMsgTo(e.target.value)}>
                <option value="">Select...</option>
                {sessions.filter(s => s.id !== msgFrom).map(s => (
                  <option key={s.id} value={s.id}>{s.name}</option>
                ))}
              </select>
            </label>
          </div>
          <label className="field">
            <span>Message</span>
            <textarea
              rows={2}
              placeholder="Message content..."
              value={msgContent}
              onChange={(e) => setMsgContent(e.target.value)}
            />
          </label>
          <div className="action-buttons">
            <button
              className="primary"
              disabled={!msgFrom || !msgTo || !msgContent.trim() || sendingMsg}
              onClick={handleSendMessage}
            >
              {sendingMsg ? 'Sending...' : 'Send Message'}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
