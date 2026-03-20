import { useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useSettings } from '../SettingsContext';
import { InfoTip } from '../shared/InfoTip';
import { notifyError, notifyWarn } from '../../../shared/notify';
import { useConfirm } from '../../../shared/useConfirm';

interface MemoryEntry {
  id: string;
  content: string;
  memory_type: string;
  tags: string[];
  created_at: string;
  last_accessed: string;
  access_count: number;
}

interface K2KSearchResult {
  title: string;
  source_type: string;
  confidence: number;
  summary: string;
  content: string;
}

interface AgentTaskState {
  task_id: string;
  status: 'pending' | 'running' | 'completed' | 'failed';
  progress: number;
  result: string | null;
  error: string | null;
}

export function KnowledgeTab() {
  const { config, setConfig, supermemoryAvailable, checkSupermemory, checkingSupermemory } = useSettings();
  const { confirm: showConfirm, modal: confirmModal } = useConfirm();

  // Memory Browser state
  const [memoryQuery, setMemoryQuery] = useState('');
  const [memoryTypeFilter, setMemoryTypeFilter] = useState('All');
  const [memories, setMemories] = useState<MemoryEntry[]>([]);
  const [searchingMemory, setSearchingMemory] = useState(false);
  const [showAddMemory, setShowAddMemory] = useState(false);
  const [newMemory, setNewMemory] = useState({ content: '', memory_type: 'Fact', tags: '' });
  const [addingMemory, setAddingMemory] = useState(false);

  // K2K Search state
  const [k2kQuery, setK2kQuery] = useState('');
  const [k2kTopK, setK2kTopK] = useState(5);
  const [k2kFederated, setK2kFederated] = useState(false);
  const [k2kResults, setK2kResults] = useState<K2KSearchResult[]>([]);
  const [searchingK2k, setSearchingK2k] = useState(false);

  // Agent Tasks state
  const [agentCapabilities, setAgentCapabilities] = useState<string[]>([]);
  const [loadingCapabilities, setLoadingCapabilities] = useState(false);
  const [selectedCapability, setSelectedCapability] = useState('');
  const [taskInput, setTaskInput] = useState('{}');
  const [taskContext, setTaskContext] = useState('');
  const [submittingTask, setSubmittingTask] = useState(false);
  const [activeTasks, setActiveTasks] = useState<AgentTaskState[]>([]);
  const [pollingTaskId, setPollingTaskId] = useState<string | null>(null);

  const handleSearchMemories = async () => {
    setSearchingMemory(true);
    try {
      let results: MemoryEntry[];
      if (memoryTypeFilter !== 'All') {
        results = await invoke<MemoryEntry[]>('get_memories_by_type', { memoryType: memoryTypeFilter });
        if (memoryQuery.trim()) {
          const q = memoryQuery.toLowerCase();
          results = results.filter(m => m.content.toLowerCase().includes(q));
        }
      } else if (memoryQuery.trim()) {
        results = await invoke<MemoryEntry[]>('search_memories', { query: memoryQuery });
      } else {
        results = await invoke<MemoryEntry[]>('search_memories', { query: '*' });
      }
      setMemories(results);
    } catch (error) {
      notifyError('Memory', `Search failed: ${error}`);
    } finally {
      setSearchingMemory(false);
    }
  };

  const handleAddMemory = async () => {
    if (!newMemory.content.trim()) return;
    setAddingMemory(true);
    try {
      const tags = newMemory.tags ? newMemory.tags.split(',').map(t => t.trim()).filter(Boolean) : [];
      await invoke('add_memory', {
        content: newMemory.content,
        memoryType: newMemory.memory_type,
        tags,
      });
      setNewMemory({ content: '', memory_type: 'Fact', tags: '' });
      setShowAddMemory(false);
      handleSearchMemories();
    } catch (error) {
      notifyError('Memory', `Failed to add memory: ${error}`);
    } finally {
      setAddingMemory(false);
    }
  };

  const handleDeleteMemory = async (id: string) => {
    if (!await showConfirm('Delete this memory?', { danger: true })) return;
    try {
      await invoke('delete_memory', { memoryId: id });
      setMemories(prev => prev.filter(m => m.id !== id));
    } catch (error) {
      notifyError('Memory', `Failed to delete: ${error}`);
    }
  };

  if (!config) return null;

  return (
    <div className="tab-content">
      {confirmModal}
      <div className="settings-group">
        <h3>K2K Integration<InfoTip text="K2K (Knowledge-to-Knowledge) connects NexiBot to the Knowledge Nexus network for federated search across devices and services." /></h3>
        <div className="inline-toggle">
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={config.k2k.enabled}
              onChange={(e) => setConfig({ ...config, k2k: { ...config.k2k, enabled: e.target.checked } })}
            />
            Enable K2K<InfoTip text="Connect to the local Knowledge Nexus System Agent for distributed knowledge access." />
          </label>
        </div>

        {config.k2k.enabled && (
          <>
            <label className="field">
              <span>Local Agent URL<InfoTip text="The WebSocket URL where your local Knowledge Nexus agent is running." /></span>
              <input
                type="text"
                value={config.k2k.local_agent_url}
                onChange={(e) => setConfig({ ...config, k2k: { ...config.k2k, local_agent_url: e.target.value } })}
              />
            </label>
            <label className="field">
              <span>Router URL (optional)<InfoTip text="Optional URL of a K2K router for connecting to remote knowledge sources." /></span>
              <input
                type="text"
                value={config.k2k.router_url || ''}
                onChange={(e) => setConfig({ ...config, k2k: { ...config.k2k, router_url: e.target.value || undefined } })}
                placeholder="http://localhost:8000"
              />
            </label>
            <div className="info-text">Client ID: {config.k2k.client_id}</div>
          </>
        )}
      </div>

      <div className="settings-group">
        <h3>Supermemory<InfoTip text="Persistent memory that survives across sessions. Conversations are extracted into searchable knowledge." /></h3>
        <p className="group-description">
          When the Knowledge Nexus System Agent is running, conversations are automatically synced as persistent supermemory. Extracted knowledge becomes searchable across sessions.
        </p>
        <div className="status-indicator">
          <span className={`status-dot ${supermemoryAvailable ? 'healthy' : 'inactive'}`} />
          <span>{supermemoryAvailable ? 'System Agent connected' : 'System Agent not detected'}</span>
          {supermemoryAvailable === false && (
            <span className="hint"> — using local-only memory</span>
          )}
        </div>
        <div className="inline-toggle">
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={config.k2k.supermemory_enabled}
              onChange={(e) => setConfig({
                ...config,
                k2k: { ...config.k2k, supermemory_enabled: e.target.checked },
              })}
            />
            Enable Supermemory (auto-sync when System Agent available)<InfoTip text="Automatically sync conversations to the System Agent's long-term memory store." />
          </label>
        </div>
        {config.k2k.supermemory_enabled && (
          <div className="inline-toggle">
            <label className="toggle-label">
              <input
                type="checkbox"
                checked={config.k2k.supermemory_auto_extract}
                onChange={(e) => setConfig({
                  ...config,
                  k2k: { ...config.k2k, supermemory_auto_extract: e.target.checked },
                })}
              />
              Auto-extract knowledge from conversations<InfoTip text="Automatically extract key facts and insights from conversations to build your knowledge base." />
            </label>
          </div>
        )}
        <button className="test-button" onClick={checkSupermemory} disabled={checkingSupermemory} style={{ marginTop: '8px' }}>
          {checkingSupermemory ? 'Checking...' : 'Check Status'}
        </button>
      </div>

      {/* Memory Browser */}
      <div className="settings-group">
        <h3>Memory Browser<InfoTip text="Search, browse, and manage NexiBot's local memory store. Memories are facts, preferences, and context extracted from conversations." /></h3>
        <p className="group-description">
          Browse and manage NexiBot's memory. Search for stored facts, preferences, and session context.
        </p>

        <div style={{ display: 'flex', gap: '8px', marginBottom: '8px' }}>
          <input
            type="text"
            placeholder="Search memories..."
            value={memoryQuery}
            onChange={(e) => setMemoryQuery(e.target.value)}
            onKeyDown={(e) => { if (e.key === 'Enter') handleSearchMemories(); }}
            style={{ flex: 1 }}
          />
          <select value={memoryTypeFilter} onChange={(e) => setMemoryTypeFilter(e.target.value)}>
            <option value="All">All Types</option>
            <option value="Fact">Fact</option>
            <option value="Preference">Preference</option>
            <option value="Session">Session</option>
            <option value="Custom">Custom</option>
          </select>
          <button className="primary" onClick={handleSearchMemories} disabled={searchingMemory}>
            {searchingMemory ? 'Searching...' : 'Search'}
          </button>
        </div>

        {memories.length > 0 && (
          <div>
            <div className="info-text" style={{ marginBottom: '8px' }}>{memories.length} result{memories.length !== 1 ? 's' : ''}</div>
            {memories.map((mem) => (
              <div key={mem.id} className="mcp-server-card">
                <div className="mcp-server-header">
                  <span className="memory-type-badge">{mem.memory_type}</span>
                  <span className="mcp-tool-count">{new Date(mem.created_at).toLocaleDateString()}</span>
                  <button className="mcp-remove-btn" onClick={() => handleDeleteMemory(mem.id)}>Delete</button>
                </div>
                <div style={{ fontSize: '13px', margin: '6px 0', whiteSpace: 'pre-wrap' }}>
                  {mem.content.length > 300 ? mem.content.substring(0, 300) + '...' : mem.content}
                </div>
                {mem.tags.length > 0 && (
                  <div className="tag-list">
                    {mem.tags.map((tag, i) => (
                      <span key={i} className="tag">{tag}</span>
                    ))}
                  </div>
                )}
              </div>
            ))}
          </div>
        )}

        {memories.length === 0 && !searchingMemory && memoryQuery && (
          <div className="info-text">No memories found. Try a different search term.</div>
        )}

        {showAddMemory ? (
          <div className="mcp-add-form" style={{ marginTop: '8px' }}>
            <label className="field">
              <span>Content<InfoTip text="The memory content to store. Be descriptive — this is what will be retrieved during conversations." /></span>
              <textarea
                rows={3}
                placeholder="What should NexiBot remember?"
                value={newMemory.content}
                onChange={(e) => setNewMemory({ ...newMemory, content: e.target.value })}
              />
            </label>
            <label className="field">
              <span>Type<InfoTip text="Categorize this memory. Facts are objective information, Preferences are user preferences, Custom is anything else." /></span>
              <select value={newMemory.memory_type} onChange={(e) => setNewMemory({ ...newMemory, memory_type: e.target.value })}>
                <option value="Fact">Fact</option>
                <option value="Preference">Preference</option>
                <option value="Custom">Custom</option>
              </select>
            </label>
            <label className="field">
              <span>Tags (comma-separated)<InfoTip text="Optional tags for organizing memories." /></span>
              <input
                type="text"
                placeholder="tag1, tag2, tag3"
                value={newMemory.tags}
                onChange={(e) => setNewMemory({ ...newMemory, tags: e.target.value })}
              />
            </label>
            <div className="mcp-add-actions">
              <button className="primary" onClick={handleAddMemory} disabled={!newMemory.content.trim() || addingMemory}>
                {addingMemory ? 'Adding...' : 'Add Memory'}
              </button>
              <button onClick={() => setShowAddMemory(false)}>Cancel</button>
            </div>
          </div>
        ) : (
          <button className="mcp-add-btn" onClick={() => setShowAddMemory(true)} style={{ marginTop: '8px' }}>
            + Add Memory
          </button>
        )}
      </div>

      {/* K2K Search */}
      {config.k2k.enabled && (
        <div className="settings-group">
          <h3>K2K Search<InfoTip text="Search across the Knowledge Nexus network. Federated mode queries remote knowledge sources in addition to local ones." /></h3>
          <p className="group-description">
            Search your local and federated knowledge sources via the K2K protocol.
          </p>

          <div style={{ display: 'flex', gap: '8px', marginBottom: '8px' }}>
            <input
              type="text"
              placeholder="Search knowledge..."
              value={k2kQuery}
              onChange={(e) => setK2kQuery(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' && k2kQuery.trim()) {
                  setSearchingK2k(true);
                  invoke<K2KSearchResult[]>('search_k2k', { query: k2kQuery, topK: k2kTopK, federated: k2kFederated })
                    .then(setK2kResults)
                    .catch((error) => notifyError('K2K', `Search failed: ${error}`))
                    .finally(() => setSearchingK2k(false));
                }
              }}
              style={{ flex: 1 }}
            />
            <input
              type="number"
              min={1}
              max={50}
              value={k2kTopK}
              onChange={(e) => setK2kTopK(parseInt(e.target.value) || 5)}
              style={{ width: '60px' }}
              title="Top K results"
            />
            <button className="primary" disabled={searchingK2k || !k2kQuery.trim()} onClick={async () => {
              setSearchingK2k(true);
              try {
                const results = await invoke<K2KSearchResult[]>('search_k2k', { query: k2kQuery, topK: k2kTopK, federated: k2kFederated });
                setK2kResults(results);
              } catch (error) {
                notifyError('K2K', `Search failed: ${error}`);
              } finally {
                setSearchingK2k(false);
              }
            }}>
              {searchingK2k ? 'Searching...' : 'Search'}
            </button>
          </div>

          <div className="inline-toggle">
            <label className="toggle-label">
              <input
                type="checkbox"
                checked={k2kFederated}
                onChange={(e) => setK2kFederated(e.target.checked)}
              />
              Federated search<InfoTip text="Include results from remote knowledge sources connected via the K2K router." />
            </label>
          </div>

          {k2kResults.length > 0 && (
            <div>
              {k2kResults.map((result, i) => (
                <div key={i} className="mcp-server-card">
                  <div className="mcp-server-header">
                    <span className="mcp-server-name">{result.title}</span>
                    <span className="mcp-server-command">{result.source_type}</span>
                    <span className="mcp-tool-count">{Math.round(result.confidence * 100)}% match</span>
                  </div>
                  {result.summary && (
                    <div style={{ fontSize: '12px', color: 'var(--text-secondary)', margin: '4px 0' }}>
                      {result.summary}
                    </div>
                  )}
                  {result.content && (
                    <div style={{ fontSize: '12px', color: 'var(--text-secondary)', margin: '4px 0', whiteSpace: 'pre-wrap' }}>
                      {result.content.length > 500 ? result.content.substring(0, 500) + '...' : result.content}
                    </div>
                  )}
                </div>
              ))}
            </div>
          )}

          {k2kResults.length === 0 && !searchingK2k && k2kQuery && (
            <div className="info-text">No results found. Try a different search term.</div>
          )}
        </div>
      )}

      {/* Agent Tasks */}
      {config.k2k.enabled && (
        <div className="settings-group">
          <h3>Agent Tasks<InfoTip text="Submit tasks to the K2K agent and monitor their progress. Tasks run asynchronously in the background." /></h3>
          <p className="group-description">
            Submit and monitor background agent tasks via the K2K protocol.
          </p>

          <div className="action-buttons">
            <button disabled={loadingCapabilities} onClick={async () => {
              setLoadingCapabilities(true);
              try {
                const caps = await invoke<string[]>('get_agent_capabilities');
                setAgentCapabilities(caps);
                if (caps.length > 0 && !selectedCapability) setSelectedCapability(caps[0]);
              } catch (error) {
                notifyError('Agent Tasks', `Failed to load capabilities: ${error}`);
              } finally {
                setLoadingCapabilities(false);
              }
            }}>
              {loadingCapabilities ? 'Loading...' : 'Load Capabilities'}
            </button>
          </div>

          {agentCapabilities.length > 0 && (
            <>
              <label className="field">
                <span>Capability<InfoTip text="Select the agent capability to invoke." /></span>
                <select value={selectedCapability} onChange={(e) => setSelectedCapability(e.target.value)}>
                  {agentCapabilities.map((cap) => (
                    <option key={cap} value={cap}>{cap}</option>
                  ))}
                </select>
              </label>
              <label className="field">
                <span>Input (JSON)<InfoTip text="JSON input for the task. The format depends on the selected capability." /></span>
                <textarea
                  rows={3}
                  placeholder='{"query": "example"}'
                  value={taskInput}
                  onChange={(e) => setTaskInput(e.target.value)}
                />
              </label>
              <label className="field">
                <span>Context (optional)<InfoTip text="Optional context string to pass along with the task." /></span>
                <input
                  type="text"
                  placeholder="Additional context..."
                  value={taskContext}
                  onChange={(e) => setTaskContext(e.target.value)}
                />
              </label>
              <div className="action-buttons">
                <button className="primary" disabled={submittingTask || !selectedCapability} onClick={async () => {
                  setSubmittingTask(true);
                  try {
                    let parsedInput;
                    try { parsedInput = JSON.parse(taskInput); } catch { notifyWarn('Agent Tasks', 'Invalid JSON input'); setSubmittingTask(false); return; }
                    const result = await invoke<{ task_id: string }>('submit_agent_task', {
                      capability: selectedCapability,
                      input: parsedInput,
                      context: taskContext || null,
                    });
                    setActiveTasks(prev => [...prev, {
                      task_id: result.task_id,
                      status: 'pending',
                      progress: 0,
                      result: null,
                      error: null,
                    }]);
                  } catch (error) {
                    notifyError('Agent Tasks', `Task submission failed: ${error}`);
                  } finally {
                    setSubmittingTask(false);
                  }
                }}>
                  {submittingTask ? 'Submitting...' : 'Submit Task'}
                </button>
              </div>
            </>
          )}

          {activeTasks.length > 0 && (
            <div style={{ marginTop: '8px' }}>
              {activeTasks.map((task) => (
                <div key={task.task_id} className="mcp-server-card">
                  <div className="mcp-server-header">
                    <span className={`status-dot ${task.status === 'completed' ? 'healthy' : task.status === 'failed' ? 'unhealthy' : 'inactive'}`}
                      style={task.status === 'pending' || task.status === 'running' ? { backgroundColor: 'var(--warning)' } : undefined} />
                    <span className="mcp-server-name">{task.task_id}</span>
                    <span className="mcp-server-command">{task.status}</span>
                    {task.progress > 0 && (
                      <span className="mcp-tool-count">{task.progress}%</span>
                    )}
                  </div>
                  {task.result && (
                    <div style={{ fontSize: '12px', color: 'var(--text-secondary)', margin: '4px 0', whiteSpace: 'pre-wrap', maxHeight: '150px', overflow: 'auto' }}>
                      {task.result}
                    </div>
                  )}
                  {task.error && (
                    <div style={{ fontSize: '12px', color: 'var(--error)', margin: '4px 0' }}>
                      {task.error}
                    </div>
                  )}
                  {(task.status === 'pending' || task.status === 'running') && (
                    <div className="action-buttons">
                      <button disabled={pollingTaskId === task.task_id} onClick={async () => {
                        setPollingTaskId(task.task_id);
                        try {
                          const status = await invoke<AgentTaskState>('poll_agent_task', { taskId: task.task_id });
                          setActiveTasks(prev => prev.map(t => t.task_id === task.task_id ? status : t));
                        } catch (error) {
                          notifyError('Agent Tasks', `Poll failed: ${error}`);
                        } finally {
                          setPollingTaskId(null);
                        }
                      }}>
                        {pollingTaskId === task.task_id ? 'Polling...' : 'Poll Status'}
                      </button>
                    </div>
                  )}
                </div>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
