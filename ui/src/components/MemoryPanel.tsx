import { useState, useEffect, useCallback, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import './MemoryPanel.css';

interface MemoryEntry {
  id: string;
  content: string;
  memory_type: string;
  tags: string[];
  created_at?: string;
}

interface Props {
  onClose: () => void;
}

// Must match backend MemoryType serde snake_case representation
const MEMORY_TYPES = ['conversation', 'preference', 'fact', 'context'];

const TYPE_LABELS: Record<string, string> = {
  preference: 'Preference',
  fact: 'Fact',
  context: 'Context',
  conversation: 'Conversation',
};

export default function MemoryPanel({ onClose }: Props) {
  const [memories, setMemories] = useState<MemoryEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [searchQuery, setSearchQuery] = useState('');
  const [deleting, setDeleting] = useState<string | null>(null);
  // Staleness guard: increment before each async fetch; only apply result if counter matches
  const fetchCounterRef = useRef(0);

  const loadAllMemories = useCallback(async () => {
    const thisRequest = ++fetchCounterRef.current;
    setLoading(true);
    setError(null);
    try {
      const results = await Promise.all(
        MEMORY_TYPES.map(t =>
          invoke<MemoryEntry[]>('get_memories_by_type', { memoryType: t }).catch(() => [] as MemoryEntry[])
        )
      );
      if (thisRequest !== fetchCounterRef.current) return;
      // Deduplicate by id in case any entries appear in multiple type buckets
      const seen = new Set<string>();
      const all = results.flat().filter(m => {
        if (seen.has(m.id)) return false;
        seen.add(m.id);
        return true;
      });
      setMemories(all);
    } catch (e) {
      if (thisRequest === fetchCounterRef.current) setError(String(e));
    } finally {
      if (thisRequest === fetchCounterRef.current) setLoading(false);
    }
  }, []);

  const searchMemories = useCallback(async (query: string) => {
    const thisRequest = ++fetchCounterRef.current;
    setLoading(true);
    setError(null);
    try {
      const result = await invoke<MemoryEntry[]>('search_memories', { query });
      if (thisRequest !== fetchCounterRef.current) return;
      setMemories(result);
    } catch (e) {
      if (thisRequest === fetchCounterRef.current) setError(String(e));
    } finally {
      if (thisRequest === fetchCounterRef.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    loadAllMemories();
  }, [loadAllMemories]);

  const handleSearchChange = (value: string) => {
    setSearchQuery(value);
    if (value.trim()) {
      searchMemories(value.trim());
    } else {
      loadAllMemories();
    }
  };

  const handleDelete = async (id: string) => {
    setDeleting(id);
    try {
      await invoke('delete_memory', { memoryId: id });
      setMemories(prev => prev.filter(m => m.id !== id));
    } catch (e) {
      setError(`Delete failed: ${e}`);
    } finally {
      setDeleting(null);
    }
  };

  return (
    <div className="memory-panel" role="dialog" aria-label="Memory panel">
      <div className="memory-panel-header">
        <h3 className="memory-panel-title">Memories</h3>
        <button className="memory-panel-close" onClick={onClose} aria-label="Close memory panel">
          ×
        </button>
      </div>

      <div className="memory-panel-search">
        <input
          type="text"
          placeholder="Search memories..."
          value={searchQuery}
          onChange={e => handleSearchChange(e.target.value)}
          className="memory-search-input"
          aria-label="Search memories"
        />
      </div>

      <div className="memory-panel-body">
        {loading && <div className="memory-loading">Loading memories...</div>}
        {error && <div className="memory-error">{error}</div>}
        {!loading && !error && memories.length === 0 && (
          <div className="memory-empty">No memories stored yet.</div>
        )}
        {!loading &&
          memories.map(m => (
            <div key={m.id} className={`memory-item memory-type-${m.memory_type.toLowerCase()}`}>
              <div className="memory-item-header">
                <span className="memory-type-badge">
                  {TYPE_LABELS[m.memory_type] ?? m.memory_type}
                </span>
                {m.tags.map(tag => (
                  <span key={tag} className="memory-tag">
                    {tag}
                  </span>
                ))}
              </div>
              <div className="memory-content">{m.content}</div>
              <div className="memory-item-actions">
                <button
                  className="memory-delete-btn"
                  onClick={() => handleDelete(m.id)}
                  disabled={deleting === m.id}
                  aria-label="Delete memory"
                >
                  {deleting === m.id ? '...' : 'Delete'}
                </button>
              </div>
            </div>
          ))}
      </div>
    </div>
  );
}
