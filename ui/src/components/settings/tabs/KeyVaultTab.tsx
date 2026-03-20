import { useState, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useSettings } from '../SettingsContext';
import type { VaultEntry } from '../SettingsContext';
import { notifyError } from '../../../shared/notify';
import { useConfirm } from '../../../shared/useConfirm';

// Shorten a proxy key for display: show prefix + first 8 chars + "..."
function abbreviateKey(key: string): string {
  if (key.length <= 28) return key;
  return key.slice(0, 28) + '…';
}

function formatDate(iso: string | null): string {
  if (!iso) return '—';
  try {
    return new Date(iso).toLocaleString();
  } catch {
    return iso;
  }
}

export function KeyVaultTab() {
  const { config, setConfig } = useSettings();
  const { confirm: showConfirm, modal: confirmModal } = useConfirm();
  const [entries, setEntries] = useState<VaultEntry[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [editingLabel, setEditingLabel] = useState<string | null>(null);
  const [labelDraft, setLabelDraft] = useState('');

  const loadEntries = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const result = await invoke<VaultEntry[]>('list_vault_entries');
      setEntries(result);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  const kvEnabled = config?.key_vault?.enabled ?? false;

  useEffect(() => {
    if (kvEnabled) {
      loadEntries();
    }
  }, [kvEnabled, loadEntries]);

  if (!config) return null;

  const kv = config.key_vault;

  const handleRevoke = async (proxyKey: string) => {
    if (!await showConfirm(`Revoke this proxy key? Tools that use it will stop working until the real key is re-entered.\n\n${abbreviateKey(proxyKey)}`, { danger: true, confirmLabel: 'Revoke' })) {
      return;
    }
    try {
      await invoke('revoke_vault_entry', { proxyKey });
      loadEntries();
    } catch (e) {
      notifyError('Key Vault', `Failed to revoke: ${e}`);
    }
  };

  const handleLabelSave = async (proxyKey: string) => {
    try {
      await invoke('label_vault_entry', { proxyKey, label: labelDraft });
      setEditingLabel(null);
      loadEntries();
    } catch (e) {
      notifyError('Key Vault', `Failed to save label: ${e}`);
    }
  };

  const handleToggle = (field: keyof typeof kv, value: boolean) => {
    setConfig({ ...config, key_vault: { ...kv, [field]: value } });
  };

  return (
    <div className="tab-content">
      {confirmModal}
      <h3>Smart Key Vault</h3>
      <p className="setting-description">
        The Key Vault intercepts real API keys at the model boundary, stores them
        encrypted locally, and gives the model format-mimicking proxy keys instead.
        Proxy keys are silently restored before tool execution — the model never
        sees real keys.
      </p>

      {/* Master enable toggle */}
      <div className="setting-group">
        <label className="toggle-label">
          <input
            type="checkbox"
            checked={kv.enabled}
            onChange={e => handleToggle('enabled', e.target.checked)}
          />
          <span>Enable Smart Key Vault</span>
        </label>
        <p className="setting-description">
          When enabled, real API keys are never visible to the model. Disabling
          this reverts to standard behaviour where the model can see real keys.
        </p>
      </div>

      {kv.enabled && (
        <>
          {/* Interception options */}
          <div className="setting-group">
            <h4>Interception Points</h4>
            <label className="toggle-label">
              <input
                type="checkbox"
                checked={kv.intercept_chat_input}
                onChange={e => handleToggle('intercept_chat_input', e.target.checked)}
              />
              <span>Intercept chat input</span>
            </label>
            <p className="setting-description">
              Scan messages you send for real API keys before the model sees them.
            </p>

            <label className="toggle-label">
              <input
                type="checkbox"
                checked={kv.intercept_config}
                onChange={e => handleToggle('intercept_config', e.target.checked)}
              />
              <span>Intercept config values</span>
            </label>
            <p className="setting-description">
              Store real keys from Settings (API key fields) in the vault; write only
              proxy keys to config.yaml.
            </p>

            <label className="toggle-label">
              <input
                type="checkbox"
                checked={kv.intercept_tool_results}
                onChange={e => handleToggle('intercept_tool_results', e.target.checked)}
              />
              <span>Intercept tool results</span>
            </label>
            <p className="setting-description">
              Replace real keys that appear in tool output (e.g., from file reads or
              environment dumps) before the model sees them.
            </p>

            <label className="toggle-label">
              <input
                type="checkbox"
                checked={kv.restore_tool_inputs}
                onChange={e => handleToggle('restore_tool_inputs', e.target.checked)}
              />
              <span>Restore proxy keys in tool inputs</span>
            </label>
            <p className="setting-description">
              When the model uses a proxy key in a tool call, restore it to the real
              key before the tool executes.
            </p>
          </div>

          {/* Vault entries table */}
          <div className="setting-group">
            <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
              <h4>Vault Entries ({entries.length})</h4>
              <button className="btn-secondary" onClick={loadEntries} disabled={loading}>
                {loading ? 'Loading…' : 'Refresh'}
              </button>
            </div>

            {error && (
              <p style={{ color: 'var(--error)', marginBottom: '8px' }}>Error: {error}</p>
            )}

            {loading ? (
              <p className="setting-description">Loading vault entries...</p>
            ) : entries.length === 0 ? (
              <p className="setting-description">
                No keys stored yet. Real API keys will appear here when they are
                first intercepted.
              </p>
            ) : (
              <div style={{ overflowX: 'auto' }}>
                <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: '13px' }}>
                  <thead>
                    <tr style={{ borderBottom: '1px solid var(--border)' }}>
                      <th style={{ textAlign: 'left', padding: '6px 8px', fontWeight: 600 }}>Proxy Key</th>
                      <th style={{ textAlign: 'left', padding: '6px 8px', fontWeight: 600 }}>Format</th>
                      <th style={{ textAlign: 'left', padding: '6px 8px', fontWeight: 600 }}>Label</th>
                      <th style={{ textAlign: 'right', padding: '6px 8px', fontWeight: 600 }}>Uses</th>
                      <th style={{ textAlign: 'left', padding: '6px 8px', fontWeight: 600 }}>Created</th>
                      <th style={{ textAlign: 'left', padding: '6px 8px', fontWeight: 600 }}>Last Used</th>
                      <th style={{ padding: '6px 8px' }}></th>
                    </tr>
                  </thead>
                  <tbody>
                    {entries.map(entry => (
                      <tr
                        key={entry.proxy_key}
                        style={{ borderBottom: '1px solid var(--border-subtle)', verticalAlign: 'middle' }}
                      >
                        <td style={{ padding: '6px 8px', fontFamily: 'monospace', fontSize: '12px', color: 'var(--text-muted)' }}>
                          {abbreviateKey(entry.proxy_key)}
                        </td>
                        <td style={{ padding: '6px 8px' }}>
                          <span style={{
                            padding: '2px 6px',
                            borderRadius: '4px',
                            background: 'var(--bg-secondary)',
                            fontSize: '11px',
                            fontWeight: 500,
                          }}>
                            {entry.format}
                          </span>
                        </td>
                        <td style={{ padding: '6px 8px' }}>
                          {editingLabel === entry.proxy_key ? (
                            <span style={{ display: 'flex', gap: '4px', alignItems: 'center' }}>
                              <input
                                type="text"
                                value={labelDraft}
                                onChange={e => setLabelDraft(e.target.value)}
                                onKeyDown={e => {
                                  if (e.key === 'Enter') handleLabelSave(entry.proxy_key);
                                  if (e.key === 'Escape') setEditingLabel(null);
                                }}
                                style={{ padding: '2px 6px', fontSize: '12px', width: '120px' }}
                                autoFocus
                              />
                              <button
                                style={{ padding: '2px 6px', fontSize: '11px' }}
                                onClick={() => handleLabelSave(entry.proxy_key)}
                              >
                                Save
                              </button>
                              <button
                                style={{ padding: '2px 6px', fontSize: '11px' }}
                                onClick={() => setEditingLabel(null)}
                              >
                                Cancel
                              </button>
                            </span>
                          ) : (
                            <span
                              style={{ cursor: 'pointer', color: entry.label ? 'var(--text)' : 'var(--text-muted)', fontSize: '12px' }}
                              title="Click to edit label"
                              onClick={() => {
                                setEditingLabel(entry.proxy_key);
                                setLabelDraft(entry.label ?? '');
                              }}
                            >
                              {entry.label ?? '+ add label'}
                            </span>
                          )}
                        </td>
                        <td style={{ padding: '6px 8px', textAlign: 'right', fontVariantNumeric: 'tabular-nums' }}>
                          {entry.use_count}
                        </td>
                        <td style={{ padding: '6px 8px', fontSize: '11px', color: 'var(--text-muted)' }}>
                          {formatDate(entry.created_at)}
                        </td>
                        <td style={{ padding: '6px 8px', fontSize: '11px', color: 'var(--text-muted)' }}>
                          {formatDate(entry.last_used)}
                        </td>
                        <td style={{ padding: '6px 8px' }}>
                          <button
                            style={{ padding: '3px 8px', fontSize: '11px', color: 'var(--error)', background: 'none', border: '1px solid var(--error)', borderRadius: '4px', cursor: 'pointer' }}
                            onClick={() => handleRevoke(entry.proxy_key)}
                            title="Revoke this proxy key"
                          >
                            Revoke
                          </button>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </div>

          {/* Info box */}
          <div style={{ padding: '12px', background: 'var(--bg-secondary)', borderRadius: '6px', fontSize: '12px', color: 'var(--text-muted)', marginTop: '8px' }}>
            <strong>How it works:</strong> When you paste an API key into chat or the Settings UI,
            it is stored encrypted in a local SQLite database. The model only sees a proxy key
            (e.g., <code>sk-ant-PROXY-xxxx…</code>). When the model uses the proxy key in a
            tool call, it is silently replaced with the real key before the tool executes.
            Revoking a proxy key renders it permanently invalid.
          </div>
        </>
      )}
    </div>
  );
}
