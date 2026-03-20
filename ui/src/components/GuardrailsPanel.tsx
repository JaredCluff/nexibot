import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { notifyError } from '../shared/notify';

interface GuardrailsConfig {
  security_level: string;
  block_destructive_commands: boolean;
  block_sensitive_data_sharing: boolean;
  detect_prompt_injection: boolean;
  block_prompt_injection: boolean;
  confirm_external_actions: boolean;
  dangers_acknowledged: boolean;
  server_permissions: Record<string, any>;
  default_tool_permission: string;
  dangerous_tool_patterns: string[];
  use_dcg: boolean;
}

interface GuardrailsPanelProps {
  onClose: () => void;
  onApplied: () => void;
}

function GuardrailsPanel({ onClose, onApplied }: GuardrailsPanelProps) {
  const [config, setConfig] = useState<GuardrailsConfig | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    (async () => {
      try {
        const cfg = await invoke<GuardrailsConfig>('get_guardrails_config');
        setConfig(cfg);
      } catch (e) {
        notifyError('Guardrails', `Failed to load configuration: ${e}`);
      } finally {
        setLoading(false);
      }
    })();
  }, []);

  const handleApply = async () => {
    if (!config) return;
    try {
      await invoke('update_guardrails_config', { newConfig: config });
      onApplied();
    } catch (e) {
      notifyError('Guardrails', `Failed to update: ${e}`);
    }
  };

  if (loading || !config) {
    return (
      <div className="guardrails-panel">
        <div className="guardrails-panel-header">
          <strong>Guardrails Configuration</strong>
        </div>
        <div className="guardrails-panel-body">Loading...</div>
      </div>
    );
  }

  return (
    <div className="guardrails-panel">
      <div className="guardrails-panel-header">
        <strong>Guardrails Configuration</strong>
        <span className="guardrails-panel-hint">Changes require your explicit confirmation.</span>
      </div>
      <div className="guardrails-panel-body">
        <label className="guardrails-field">
          <span>Security Level</span>
          <select
            value={config.security_level}
            onChange={(e) => setConfig({ ...config, security_level: e.target.value })}
          >
            <option value="Maximum">Maximum</option>
            <option value="Standard">Standard (recommended)</option>
            <option value="Relaxed">Relaxed</option>
            <option value="Disabled">Disabled</option>
          </select>
        </label>

        <label className="guardrails-toggle">
          <input
            type="checkbox"
            checked={config.block_destructive_commands}
            onChange={(e) => setConfig({ ...config, block_destructive_commands: e.target.checked })}
          />
          Block destructive commands
        </label>

        <label className="guardrails-toggle">
          <input
            type="checkbox"
            checked={config.block_sensitive_data_sharing}
            onChange={(e) => setConfig({ ...config, block_sensitive_data_sharing: e.target.checked })}
          />
          Block sensitive data sharing
        </label>

        <label className="guardrails-toggle">
          <input
            type="checkbox"
            checked={config.detect_prompt_injection}
            onChange={(e) => setConfig({ ...config, detect_prompt_injection: e.target.checked })}
          />
          Detect prompt injection
        </label>

        <label className="guardrails-toggle">
          <input
            type="checkbox"
            checked={config.block_prompt_injection}
            onChange={(e) => setConfig({ ...config, block_prompt_injection: e.target.checked })}
          />
          Block prompt injection
        </label>

        <label className="guardrails-toggle">
          <input
            type="checkbox"
            checked={config.confirm_external_actions}
            onChange={(e) => setConfig({ ...config, confirm_external_actions: e.target.checked })}
          />
          Require confirmation for external actions
        </label>

        <label className="guardrails-field">
          <span>Default Tool Permission</span>
          <select
            value={config.default_tool_permission}
            onChange={(e) => setConfig({ ...config, default_tool_permission: e.target.value })}
          >
            <option value="AutoApprove">Auto-Approve</option>
            <option value="AllowWithLogging">Allow with Logging</option>
            <option value="RequireConfirmation">Require Confirmation</option>
            <option value="Block">Block All</option>
          </select>
        </label>
      </div>
      <div className="guardrails-panel-actions">
        <button className="guardrails-apply" onClick={handleApply}>Apply</button>
        <button className="guardrails-cancel" onClick={onClose}>Cancel</button>
      </div>
    </div>
  );
}

export default GuardrailsPanel;
