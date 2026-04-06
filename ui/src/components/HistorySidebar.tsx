import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { notifyError } from '../shared/notify';
import './HistorySidebar.css';

interface ConversationSession {
  id: string;
  title: string | null;
  started_at: string;
  last_activity: string;
  messages: { role: string; content: string; timestamp: string }[];
}

interface HistorySidebarProps {
  isOpen: boolean;
  onToggle: () => void;
  onSessionSelect: (session: ConversationSession) => void;
  onNewConversation: () => void;
  currentSessionId?: string;
}

function getRelativeDate(dateStr: string): string {
  const date = new Date(dateStr);
  const now = new Date();
  const diffMs = now.getTime() - date.getTime();
  const diffMins = Math.floor(diffMs / 60000);
  const diffHours = Math.floor(diffMs / 3600000);
  const diffDays = Math.floor(diffMs / 86400000);

  if (diffMins < 1) return 'Just now';
  if (diffMins < 60) return `${diffMins}m ago`;
  if (diffHours < 24) return `${diffHours}h ago`;
  if (diffDays === 1) return 'Yesterday';
  if (diffDays < 7) return `${diffDays}d ago`;
  return date.toLocaleDateString();
}

function getDateGroup(dateStr: string): string {
  const date = new Date(dateStr);
  const now = new Date();
  const diffMs = now.getTime() - date.getTime();
  const diffDays = Math.floor(diffMs / 86400000);

  if (diffDays === 0) return 'Today';
  if (diffDays === 1) return 'Yesterday';
  if (diffDays < 7) return 'This Week';
  return 'Older';
}

function HistorySidebar({ isOpen, onSessionSelect, onNewConversation, currentSessionId }: HistorySidebarProps) {
  const [sessions, setSessions] = useState<ConversationSession[]>([]);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    if (isOpen) {
      loadSessions();
    }
  }, [isOpen]);

  const loadSessions = async () => {
    setLoading(true);
    try {
      const result = await invoke<ConversationSession[]>('list_conversation_sessions');
      // Sort by last_activity descending
      result.sort((a, b) => new Date(b.last_activity).getTime() - new Date(a.last_activity).getTime());
      setSessions(result);
    } catch (error) {
      notifyError('History', `Failed to load sessions: ${error}`);
    } finally {
      setLoading(false);
    }
  };

  if (!isOpen) {
    return null;
  }

  // Group sessions by date category
  const grouped: Record<string, ConversationSession[]> = {};
  for (const session of sessions) {
    const group = getDateGroup(session.last_activity);
    if (!grouped[group]) grouped[group] = [];
    grouped[group].push(session);
  }

  const groupOrder = ['Today', 'Yesterday', 'This Week', 'Older'];

  return (
    <nav className="history-sidebar" aria-label="Conversation history">
      <div className="sidebar-header">
        <span className="sidebar-title">History</span>
      </div>

      <button className="new-conversation-btn" onClick={onNewConversation}>
        + New Conversation
      </button>

      <div className="sessions-list">
        {loading && sessions.length === 0 && (
          <div className="sidebar-loading" role="status">Loading...</div>
        )}

        {!loading && sessions.length === 0 && (
          <div className="sidebar-empty" role="status">No conversations yet</div>
        )}

        {groupOrder.map(group => {
          const items = grouped[group];
          if (!items || items.length === 0) return null;
          return (
            <div key={group} className="session-group">
              <div className="session-group-label">{group}</div>
              {items.map(session => (
                <button
                  key={session.id}
                  className={`session-item ${session.id === currentSessionId ? 'active' : ''}`}
                  aria-current={session.id === currentSessionId ? 'true' : undefined}
                  onClick={() => onSessionSelect(session)}
                >
                  <span className="session-title">
                    {session.title || 'Untitled'}
                  </span>
                  <span className="session-meta">
                    <span className="session-date">{getRelativeDate(session.last_activity)}</span>
                    <span className="session-count">{session.messages.length} msgs</span>
                  </span>
                </button>
              ))}
            </div>
          );
        })}
      </div>
    </nav>
  );
}

export default HistorySidebar;
