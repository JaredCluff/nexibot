import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { notifyError } from '../shared/notify';

type AutonomyLevel = 'Autonomous' | 'AskUser' | 'Blocked';

interface FilesystemAutonomy {
  read: AutonomyLevel;
  write: AutonomyLevel;
  delete: AutonomyLevel;
}

interface ExecuteAutonomy {
  run_command: AutonomyLevel;
  run_python: AutonomyLevel;
  run_node: AutonomyLevel;
}

interface FetchAutonomy {
  get_requests: AutonomyLevel;
  post_requests: AutonomyLevel;
}

interface BrowserAutonomy {
  navigate: AutonomyLevel;
  interact: AutonomyLevel;
}

interface CapabilityAutonomy {
  level: AutonomyLevel;
}

interface AutonomousModeConfig {
  enabled: boolean;
  filesystem: FilesystemAutonomy;
  execute: ExecuteAutonomy;
  fetch: FetchAutonomy;
  browser: BrowserAutonomy;
  computer_use: CapabilityAutonomy;
  mcp: Record<string, CapabilityAutonomy>;
  settings_modification: CapabilityAutonomy;
  memory_modification: CapabilityAutonomy;
  soul_modification: CapabilityAutonomy;
  nats_publish: CapabilityAutonomy;
}

const LEVEL_COLORS: Record<AutonomyLevel, string> = {
  Autonomous: '#22c55e',
  AskUser: '#eab308',
  Blocked: '#ef4444',
};

const LEVEL_LABELS: Record<AutonomyLevel, string> = {
  Autonomous: 'Auto',
  AskUser: 'Ask',
  Blocked: 'Block',
};

function LevelSelector({ value, onChange, disabled }: {
  value: AutonomyLevel;
  onChange: (level: AutonomyLevel) => void;
  disabled?: boolean;
}) {
  const levels: AutonomyLevel[] = ['Autonomous', 'AskUser', 'Blocked'];
  return (
    <div className="autonomy-level-selector">
      {levels.map((level) => (
        <button
          key={level}
          className={`autonomy-level-btn ${value === level ? 'active' : ''}`}
          style={{
            backgroundColor: value === level ? LEVEL_COLORS[level] : 'transparent',
            color: value === level ? '#fff' : '#999',
            borderColor: LEVEL_COLORS[level],
          }}
          onClick={() => !disabled && onChange(level)}
          disabled={disabled}
          title={level === 'Autonomous' ? 'Do it without asking' : level === 'AskUser' ? 'Ask before doing' : 'Never do this'}
        >
          {LEVEL_LABELS[level]}
        </button>
      ))}
    </div>
  );
}

function CapabilityRow({ label, description, value, onChange, disabled }: {
  label: string;
  description?: string;
  value: AutonomyLevel;
  onChange: (level: AutonomyLevel) => void;
  disabled?: boolean;
}) {
  return (
    <div className="autonomy-capability-row">
      <div className="autonomy-capability-info">
        <span className="autonomy-capability-label">{label}</span>
        {description && <span className="autonomy-capability-desc">{description}</span>}
      </div>
      <LevelSelector value={value} onChange={onChange} disabled={disabled} />
    </div>
  );
}

function AutonomousModePanel() {
  const [config, setConfig] = useState<AutonomousModeConfig | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [dirty, setDirty] = useState(false);

  useEffect(() => {
    loadConfig();
  }, []);

  const loadConfig = async () => {
    try {
      const cfg = await invoke<AutonomousModeConfig>('get_autonomous_config');
      setConfig(cfg);
      setDirty(false);
    } catch (e) {
      notifyError('Autonomous Mode', `Failed to load config: ${e}`);
    } finally {
      setLoading(false);
    }
  };

  const saveConfig = async () => {
    if (!config) return;
    setSaving(true);
    try {
      await invoke('update_autonomous_config', { newConfig: config });
      setDirty(false);
    } catch (e) {
      notifyError('Autonomous Mode', `Failed to save: ${e}`);
    } finally {
      setSaving(false);
    }
  };

  const update = (updater: (cfg: AutonomousModeConfig) => AutonomousModeConfig) => {
    if (!config) return;
    setConfig(updater(config));
    setDirty(true);
  };

  if (loading || !config) {
    return <div className="autonomy-panel">Loading autonomous mode config...</div>;
  }

  const disabled = !config.enabled;

  return (
    <div className="autonomy-panel">
      {/* Master toggle */}
      <div className="autonomy-master-toggle">
        <label className="toggle-label">
          <input
            type="checkbox"
            checked={config.enabled}
            onChange={(e) => update(c => ({ ...c, enabled: e.target.checked }))}
          />
          <strong>Enable Autonomous Mode</strong>
        </label>
        <p className="autonomy-master-desc">
          When enabled, the agent will act on its own for capabilities set to "Auto",
          refuse capabilities set to "Block", and ask for confirmation on "Ask" (channels without an approval path will block).
        </p>
      </div>

      {disabled && (
        <div className="autonomy-disabled-overlay">
          <p>Autonomous mode is disabled. Enable it above to configure per-capability permissions.</p>
        </div>
      )}

      {/* Filesystem */}
      <div className={`autonomy-group ${disabled ? 'dimmed' : ''}`}>
        <h4>Filesystem</h4>
        <CapabilityRow label="Read files" description="read_file, file_info, list_directory" value={config.filesystem.read} onChange={(l) => update(c => ({ ...c, filesystem: { ...c.filesystem, read: l } }))} disabled={disabled} />
        <CapabilityRow label="Write files" description="write_file, create_directory" value={config.filesystem.write} onChange={(l) => update(c => ({ ...c, filesystem: { ...c.filesystem, write: l } }))} disabled={disabled} />
        <CapabilityRow label="Delete files" description="delete_file" value={config.filesystem.delete} onChange={(l) => update(c => ({ ...c, filesystem: { ...c.filesystem, delete: l } }))} disabled={disabled} />
      </div>

      {/* Execution */}
      <div className={`autonomy-group ${disabled ? 'dimmed' : ''}`}>
        <h4>Code Execution</h4>
        <CapabilityRow label="Shell commands" description="run_command" value={config.execute.run_command} onChange={(l) => update(c => ({ ...c, execute: { ...c.execute, run_command: l } }))} disabled={disabled} />
        <CapabilityRow label="Python scripts" description="run_python" value={config.execute.run_python} onChange={(l) => update(c => ({ ...c, execute: { ...c.execute, run_python: l } }))} disabled={disabled} />
        <CapabilityRow label="Node.js scripts" description="run_node" value={config.execute.run_node} onChange={(l) => update(c => ({ ...c, execute: { ...c.execute, run_node: l } }))} disabled={disabled} />
      </div>

      {/* Web */}
      <div className={`autonomy-group ${disabled ? 'dimmed' : ''}`}>
        <h4>Web / Network</h4>
        <CapabilityRow label="GET requests" description="Fetch web pages, read APIs" value={config.fetch.get_requests} onChange={(l) => update(c => ({ ...c, fetch: { ...c.fetch, get_requests: l } }))} disabled={disabled} />
        <CapabilityRow label="POST requests" description="Submit data, write APIs" value={config.fetch.post_requests} onChange={(l) => update(c => ({ ...c, fetch: { ...c.fetch, post_requests: l } }))} disabled={disabled} />
      </div>

      {/* Browser */}
      <div className={`autonomy-group ${disabled ? 'dimmed' : ''}`}>
        <h4>Browser Automation</h4>
        <CapabilityRow label="Navigate" description="Go to URLs" value={config.browser.navigate} onChange={(l) => update(c => ({ ...c, browser: { ...c.browser, navigate: l } }))} disabled={disabled} />
        <CapabilityRow label="Interact" description="Click, type, scroll" value={config.browser.interact} onChange={(l) => update(c => ({ ...c, browser: { ...c.browser, interact: l } }))} disabled={disabled} />
      </div>

      {/* Computer Use */}
      <div className={`autonomy-group ${disabled ? 'dimmed' : ''}`}>
        <h4>Computer Use</h4>
        <CapabilityRow label="Screen control" description="Mouse, keyboard, screenshots" value={config.computer_use.level} onChange={(l) => update(c => ({ ...c, computer_use: { level: l } }))} disabled={disabled} />
      </div>

      {/* Self-Modification */}
      <div className={`autonomy-group ${disabled ? 'dimmed' : ''}`}>
        <h4>Self-Modification</h4>
        <CapabilityRow label="Settings" description="Modify NexiBot configuration" value={config.settings_modification.level} onChange={(l) => update(c => ({ ...c, settings_modification: { level: l } }))} disabled={disabled} />
        <CapabilityRow label="Memory" description="Add/modify persistent memories" value={config.memory_modification.level} onChange={(l) => update(c => ({ ...c, memory_modification: { level: l } }))} disabled={disabled} />
        <CapabilityRow label="Soul / Personality" description="Modify SOUL.md personality" value={config.soul_modification.level} onChange={(l) => update(c => ({ ...c, soul_modification: { level: l } }))} disabled={disabled} />
        <CapabilityRow label="NATS Publish" description="Publish messages to NATS subjects" value={config.nats_publish.level} onChange={(l) => update(c => ({ ...c, nats_publish: { level: l } }))} disabled={disabled} />
      </div>

      {/* Hard Limits (read-only) */}
      <div className="autonomy-group autonomy-hard-limits">
        <h4>Hard Safety Limits (always enforced)</h4>
        <ul>
          <li>Catastrophic commands (destructive disk operations, fork bombs) are always blocked</li>
          <li>System directories (OS-critical paths) are never accessible</li>
          <li>API keys, passwords, private keys, and credit cards are never exposed</li>
          <li>Config file cannot be modified or deleted directly</li>
        </ul>
      </div>

      {/* Save button */}
      {dirty && (
        <div className="autonomy-save-bar">
          <button className="autonomy-save-btn" onClick={saveConfig} disabled={saving}>
            {saving ? 'Saving...' : 'Save Changes'}
          </button>
          <button className="autonomy-cancel-btn" onClick={loadConfig}>Discard</button>
        </div>
      )}
    </div>
  );
}

export default AutonomousModePanel;
