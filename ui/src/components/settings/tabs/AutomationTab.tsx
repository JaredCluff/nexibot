import { useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useSettings } from '../SettingsContext';
import { InfoTip } from '../shared/InfoTip';
import type { TaskExecutionResult, WebhookEndpoint } from '../SettingsContext';
import { notifyError } from '../../../shared/notify';
import { useConfirm } from '../../../shared/useConfirm';

export function AutomationTab() {
  const { config, setConfig, scheduledTasks, schedulerEnabled, setSchedulerEnabled, schedulerResults, loadSchedulerData } = useSettings();
  const { confirm: showConfirm, modal: confirmModal } = useConfirm();

  const [showAddTask, setShowAddTask] = useState(false);
  const [newTask, setNewTask] = useState({ name: '', schedule: 'daily 09:00', prompt: '' });
  const [showResults, setShowResults] = useState(false);
  const [showAddEndpoint, setShowAddEndpoint] = useState(false);
  const [newEndpoint, setNewEndpoint] = useState({ name: '', action: 'trigger_task', target: '' });

  if (!config) return null;

  const handleAddScheduledTask = async () => {
    if (!newTask.name || !newTask.prompt) return;
    try {
      await invoke('add_scheduled_task', {
        name: newTask.name,
        schedule: newTask.schedule,
        prompt: newTask.prompt,
      });
      setNewTask({ name: '', schedule: 'daily 09:00', prompt: '' });
      setShowAddTask(false);
      loadSchedulerData();
    } catch (error) {
      notifyError('Scheduler', `Failed to add task: ${error}`);
    }
  };

  const handleRemoveScheduledTask = async (taskId: string) => {
    if (!await showConfirm('Remove this scheduled task?', { danger: true })) return;
    try {
      await invoke('remove_scheduled_task', { taskId });
      loadSchedulerData();
    } catch (error) {
      notifyError('Scheduler', `Failed to remove task: ${error}`);
    }
  };

  const handleToggleScheduledTask = async (taskId: string, enabled: boolean) => {
    try {
      await invoke('update_scheduled_task', { taskId, enabled });
      loadSchedulerData();
    } catch (error) {
      notifyError('Scheduler', `Failed to update task: ${error}`);
    }
  };

  const handleTriggerScheduledTask = async (taskId: string) => {
    try {
      await invoke<TaskExecutionResult>('trigger_scheduled_task', { taskId });
      loadSchedulerData();
    } catch (error) {
      notifyError('Scheduler', `Failed to run task: ${error}`);
    }
  };

  const handleSetSchedulerEnabled = async (enabled: boolean) => {
    try {
      await invoke('set_scheduler_enabled', { enabled });
      setSchedulerEnabled(enabled);
    } catch (error) {
      notifyError('Scheduler', `Failed to toggle scheduler: ${error}`);
    }
  };

  return (
    <div className="tab-content">
      {confirmModal}
      {/* Scheduler */}
      <div className="settings-group">
        <h3>Scheduler<InfoTip text="Run tasks automatically on a schedule. NexiBot sends the task prompt to Claude at the configured times." /></h3>
        <p className="group-description">
          Schedule tasks to run automatically. NexiBot will send the task prompt to Claude at the specified times.
        </p>
        <div className="inline-toggle">
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={schedulerEnabled}
              onChange={(e) => handleSetSchedulerEnabled(e.target.checked)}
            />
            Enable Scheduler<InfoTip text="Turn on the task scheduler. Scheduled tasks run in the background at their configured times." />
          </label>
        </div>
      </div>

      <div className="settings-group">
        <h3>Tasks</h3>
        {scheduledTasks.length === 0 && !showAddTask && (
          <div className="info-text">No scheduled tasks. Add one to get started.</div>
        )}

        {scheduledTasks.map((task) => (
          <div key={task.id} className="mcp-server-card">
            <div className="mcp-server-header">
              <span className={`mcp-status-dot`} style={{ backgroundColor: task.enabled ? 'var(--success)' : 'var(--text-secondary)' }} />
              <span className="mcp-server-name">{task.name}</span>
              <span className="mcp-server-command">{task.schedule}</span>
              <span className="mcp-tool-count">
                {task.last_run ? `Last: ${new Date(task.last_run).toLocaleString()}` : 'Never run'}
              </span>
            </div>
            <div style={{ fontSize: '12px', color: 'var(--text-secondary)', margin: '6px 0', whiteSpace: 'pre-wrap' }}>
              {task.prompt.length > 120 ? task.prompt.substring(0, 120) + '...' : task.prompt}
            </div>
            <div className="action-buttons">
              <button onClick={() => handleToggleScheduledTask(task.id, !task.enabled)}>
                {task.enabled ? 'Disable' : 'Enable'}
              </button>
              <button className="primary" onClick={() => handleTriggerScheduledTask(task.id)}>
                Run Now
              </button>
              <button className="danger" onClick={() => handleRemoveScheduledTask(task.id)}>
                Remove
              </button>
            </div>
          </div>
        ))}

        {showAddTask ? (
          <div className="mcp-add-form">
            <label className="field">
              <span>Task Name<InfoTip text="A descriptive name for this scheduled task." /></span>
              <input
                type="text"
                placeholder="Task name (e.g., Morning Summary)"
                value={newTask.name}
                onChange={(e) => setNewTask({ ...newTask, name: e.target.value })}
              />
            </label>
            <label className="field">
              <span>Schedule<InfoTip text="How often to run this task. Choose a preset or enter a custom schedule." /></span>
              <select
                value={newTask.schedule}
                onChange={(e) => setNewTask({ ...newTask, schedule: e.target.value })}
              >
                <option value="hourly">Hourly</option>
                <option value="every 30m">Every 30 minutes</option>
                <option value="every 15m">Every 15 minutes</option>
                <option value="daily 09:00">Daily at 9:00 AM</option>
                <option value="daily 12:00">Daily at 12:00 PM</option>
                <option value="daily 17:00">Daily at 5:00 PM</option>
                <option value="weekly mon 09:00">Weekly on Monday at 9:00 AM</option>
                <option value="weekly fri 17:00">Weekly on Friday at 5:00 PM</option>
              </select>
              <input
                type="text"
                placeholder="Or enter custom schedule (e.g., daily 08:30, every 5m)"
                onChange={(e) => {
                  if (e.target.value) setNewTask({ ...newTask, schedule: e.target.value });
                }}
              />
            </label>
            <label className="field">
              <span>Task Prompt<InfoTip text="The instruction to send to Claude when this task runs. Be specific about what you want done." /></span>
              <textarea
                placeholder="Task prompt — what should NexiBot do? (e.g., Check my calendar and summarize today's meetings)"
                value={newTask.prompt}
                onChange={(e) => setNewTask({ ...newTask, prompt: e.target.value })}
                rows={3}
              />
            </label>
            <div className="mcp-add-actions">
              <button onClick={handleAddScheduledTask} disabled={!newTask.name || !newTask.prompt}>
                Add Task
              </button>
              <button onClick={() => setShowAddTask(false)}>Cancel</button>
            </div>
          </div>
        ) : (
          <button className="mcp-add-btn" onClick={() => setShowAddTask(true)}>
            + Add Scheduled Task
          </button>
        )}
      </div>

      {schedulerResults.length > 0 && (
        <div className="settings-group">
          <h3 onClick={() => setShowResults(!showResults)} style={{ cursor: 'pointer' }}>
            Recent Results {showResults ? '(hide)' : `(${schedulerResults.length})`}
          </h3>
          {showResults && (
            <div>
              {schedulerResults.slice().reverse().map((result, i) => (
                <div key={i} className="mcp-server-card">
                  <div className="mcp-server-header">
                    <span className="mcp-status-dot" style={{ backgroundColor: result.success ? 'var(--success)' : 'var(--error)' }} />
                    <span className="mcp-server-name">{result.task_name}</span>
                    <span className="mcp-tool-count">{new Date(result.timestamp).toLocaleString()}</span>
                  </div>
                  <div style={{ fontSize: '12px', color: 'var(--text-secondary)', margin: '6px 0', whiteSpace: 'pre-wrap', maxHeight: '100px', overflow: 'auto' }}>
                    {result.response.length > 300 ? result.response.substring(0, 300) + '...' : result.response}
                  </div>
                </div>
              ))}
            </div>
          )}
        </div>
      )}

      {/* Webhooks */}
      <div className="settings-group">
        <h3>Webhooks<InfoTip text="Receive HTTP requests from external services to trigger tasks or send messages to Claude." /></h3>
        <p className="group-description">
          Receive HTTP requests to trigger tasks or send messages to Claude from external services.
        </p>
        <div className="inline-toggle">
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={config.webhooks?.enabled || false}
              onChange={async (e) => {
                const checked = e.target.checked;
                try {
                  await invoke('set_webhook_enabled', { enabled: checked });
                  setConfig({
                    ...config,
                    webhooks: { ...config.webhooks, enabled: checked },
                  });
                } catch (error) {
                  notifyError('Webhooks', `Failed to update webhook server: ${error}`);
                }
              }}
            />
            Enable Webhook Server<InfoTip text="Start an HTTP server that listens for incoming webhook requests on the configured port." />
          </label>
          {config.webhooks?.enabled && (
            <span className="hint">Port: {config.webhooks.port}</span>
          )}
        </div>

        {config.webhooks?.enabled && (
          <>
            <div style={{ margin: '8px 0' }}>
              <label className="field">
                <span>Bearer Token<InfoTip text="Authentication token required in webhook requests. Include as 'Authorization: Bearer TOKEN' header." /></span>
                <div style={{ display: 'flex', gap: '8px', alignItems: 'center' }}>
                  <input
                    type="text"
                    value={config.webhooks.auth_token || '(not set)'}
                    readOnly
                    style={{ fontFamily: 'monospace', fontSize: '12px', flex: 1 }}
                  />
                  <button className="test-button" onClick={async () => {
                    if (config.webhooks.auth_token) {
                      try { await navigator.clipboard.writeText(config.webhooks.auth_token); }
                      catch { notifyError('Webhooks', 'Failed to copy token to clipboard'); }
                    }
                  }}>Copy</button>
                  <button className="test-button" onClick={async () => {
                    try {
                      const token = await invoke<string>('regenerate_webhook_token');
                      setConfig({
                        ...config,
                        webhooks: { ...config.webhooks, auth_token: token },
                      });
                    } catch (error) {
                      notifyError('Webhooks', `Failed to regenerate token: ${error}`);
                    }
                  }}>Regenerate</button>
                </div>
              </label>
            </div>

            <h4>Endpoints</h4>
            {(!config.webhooks.endpoints || config.webhooks.endpoints.length === 0) && !showAddEndpoint && (
              <div className="info-text">No webhook endpoints configured.</div>
            )}

            {config.webhooks.endpoints?.map((ep) => (
              <div key={ep.id} className="mcp-server-card">
                <div className="mcp-server-header">
                  <span className="mcp-server-name">{ep.name}</span>
                  <span className="mcp-server-command">{ep.action === 'TriggerTask' ? 'Trigger Task' : 'Send Message'}</span>
                  <span className="mcp-tool-count" style={{ fontFamily: 'monospace', fontSize: '11px' }}>
                    /webhook/{ep.id}
                  </span>
                  <button className="mcp-remove-btn" onClick={async () => {
                    try {
                      await invoke('remove_webhook_endpoint', { endpointId: ep.id });
                      setConfig({
                        ...config,
                        webhooks: {
                          ...config.webhooks,
                          endpoints: config.webhooks.endpoints.filter(e => e.id !== ep.id),
                        },
                      });
                    } catch (error) {
                      notifyError('Webhooks', `Failed to remove endpoint: ${error}`);
                    }
                  }}>Remove</button>
                </div>
              </div>
            ))}

            {showAddEndpoint ? (
              <div className="mcp-add-form">
                <label className="field">
                  <span>Endpoint Name<InfoTip text="A name for this webhook endpoint." /></span>
                  <input
                    type="text"
                    placeholder="Endpoint name"
                    value={newEndpoint.name}
                    onChange={(e) => setNewEndpoint({ ...newEndpoint, name: e.target.value })}
                  />
                </label>
                <label className="field">
                  <span>Action Type<InfoTip text="What happens when this endpoint receives a request. 'Trigger Task' runs a scheduled task; 'Send Message' sends the request content to Claude." /></span>
                  <select
                    value={newEndpoint.action}
                    onChange={(e) => setNewEndpoint({ ...newEndpoint, action: e.target.value })}
                  >
                    <option value="trigger_task">Trigger Task</option>
                    <option value="send_message">Send Message</option>
                  </select>
                </label>
                {newEndpoint.action === 'trigger_task' ? (
                  <select
                    value={newEndpoint.target}
                    onChange={(e) => setNewEndpoint({ ...newEndpoint, target: e.target.value })}
                  >
                    <option value="">Select a task...</option>
                    {scheduledTasks.map((task) => (
                      <option key={task.id} value={task.id}>{task.name}</option>
                    ))}
                  </select>
                ) : (
                  <textarea
                    placeholder="Prompt template (use {{body}} for request body)"
                    value={newEndpoint.target}
                    onChange={(e) => setNewEndpoint({ ...newEndpoint, target: e.target.value })}
                    rows={2}
                  />
                )}
                <div className="mcp-add-actions">
                  <button
                    disabled={!newEndpoint.name || !newEndpoint.target}
                    onClick={async () => {
                      try {
                        const ep = await invoke<WebhookEndpoint>('add_webhook_endpoint', {
                          name: newEndpoint.name,
                          action: newEndpoint.action,
                          target: newEndpoint.target,
                        });
                        setConfig({
                          ...config,
                          webhooks: {
                            ...config.webhooks,
                            endpoints: [...(config.webhooks.endpoints || []), ep],
                          },
                        });
                        setNewEndpoint({ name: '', action: 'trigger_task', target: '' });
                        setShowAddEndpoint(false);
                      } catch (error) {
                        notifyError('Webhooks', `Failed to add endpoint: ${error}`);
                      }
                    }}
                  >Add Endpoint</button>
                  <button onClick={() => setShowAddEndpoint(false)}>Cancel</button>
                </div>
              </div>
            ) : (
              <button className="mcp-add-btn" onClick={() => setShowAddEndpoint(true)}>
                + Add Webhook Endpoint
              </button>
            )}

            <h4 style={{ marginTop: '16px' }}>TLS <InfoTip text="Enable HTTPS on the webhook server. Can auto-generate a self-signed certificate for testing." /></h4>
            <div className="inline-toggle">
              <label className="toggle-label">
                <input
                  type="checkbox"
                  checked={config.webhooks.tls?.enabled ?? false}
                  onChange={(e) => setConfig({
                    ...config,
                    webhooks: {
                      ...config.webhooks,
                      tls: { ...(config.webhooks.tls ?? { enabled: false, auto_generate: true }), enabled: e.target.checked },
                    },
                  })}
                />
                Enable TLS (HTTPS)
              </label>
            </div>
            {config.webhooks.tls?.enabled && (
              <>
                <div className="inline-toggle">
                  <label className="toggle-label">
                    <input
                      type="checkbox"
                      checked={config.webhooks.tls.auto_generate ?? true}
                      onChange={(e) => setConfig({
                        ...config,
                        webhooks: {
                          ...config.webhooks,
                          tls: { ...config.webhooks.tls, auto_generate: e.target.checked },
                        },
                      })}
                    />
                    Auto-generate self-signed certificate <InfoTip text="Automatically generate a self-signed certificate if none exists. Good for testing, but browsers will show a warning." />
                  </label>
                </div>
                <label className="field">
                  <span>Certificate Path <InfoTip text="Path to your TLS certificate PEM file. Leave empty to use the default location." /></span>
                  <input
                    type="text"
                    value={config.webhooks.tls.cert_path || ''}
                    placeholder="Leave empty for default location"
                    onChange={(e) => setConfig({
                      ...config,
                      webhooks: {
                        ...config.webhooks,
                        tls: { ...config.webhooks.tls, cert_path: e.target.value || undefined },
                      },
                    })}
                  />
                </label>
                <label className="field">
                  <span>Key Path <InfoTip text="Path to your TLS private key PEM file. Leave empty to use the default location." /></span>
                  <input
                    type="text"
                    value={config.webhooks.tls.key_path || ''}
                    placeholder="Leave empty for default location"
                    onChange={(e) => setConfig({
                      ...config,
                      webhooks: {
                        ...config.webhooks,
                        tls: { ...config.webhooks.tls, key_path: e.target.value || undefined },
                      },
                    })}
                  />
                </label>
              </>
            )}

            <h4 style={{ marginTop: '16px' }}>Rate Limiting <InfoTip text="Protect against brute-force authentication attacks by limiting failed attempts per IP address." /></h4>
            <div className="settings-row">
              <label className="field">
                <span>Max Attempts <InfoTip text="Maximum failed auth attempts before an IP is temporarily blocked." /></span>
                <input
                  type="number"
                  min="1"
                  value={config.webhooks.rate_limit?.max_attempts ?? 10}
                  onChange={(e) => setConfig({
                    ...config,
                    webhooks: {
                      ...config.webhooks,
                      rate_limit: { ...(config.webhooks.rate_limit ?? { max_attempts: 10, window_secs: 60, lockout_secs: 300 }), max_attempts: parseInt(e.target.value) || 10 },
                    },
                  })}
                />
              </label>
              <label className="field">
                <span>Window (sec) <InfoTip text="Time window in seconds over which failed attempts are counted." /></span>
                <input
                  type="number"
                  min="10"
                  value={config.webhooks.rate_limit?.window_secs ?? 60}
                  onChange={(e) => setConfig({
                    ...config,
                    webhooks: {
                      ...config.webhooks,
                      rate_limit: { ...(config.webhooks.rate_limit ?? { max_attempts: 10, window_secs: 60, lockout_secs: 300 }), window_secs: parseInt(e.target.value) || 60 },
                    },
                  })}
                />
              </label>
              <label className="field">
                <span>Lockout (sec) <InfoTip text="How long a blocked IP stays blocked, in seconds." /></span>
                <input
                  type="number"
                  min="30"
                  value={config.webhooks.rate_limit?.lockout_secs ?? 300}
                  onChange={(e) => setConfig({
                    ...config,
                    webhooks: {
                      ...config.webhooks,
                      rate_limit: { ...(config.webhooks.rate_limit ?? { max_attempts: 10, window_secs: 60, lockout_secs: 300 }), lockout_secs: parseInt(e.target.value) || 300 },
                    },
                  })}
                />
              </label>
            </div>
          </>
        )}
      </div>

    </div>
  );
}
