import { useState, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useSettings } from '../SettingsContext';
import { InfoTip } from '../shared/InfoTip';
import { TagInput } from '../shared/TagInput';
import { notifyError, notifySuccess } from '../../../shared/notify';
import { useConfirm } from '../../../shared/useConfirm';

interface GatedShellStatus {
  enabled: boolean;
  active_sessions: number;
  secret_count: number;
  debug_mode: boolean;
}

interface PatternDraft {
  name: string;
  pattern: string;
  format: string;
}

export function GatedShellTab() {
  const { config, setConfig } = useSettings();
  const { confirm: showConfirm, modal: confirmModal } = useConfirm();
  const [status, setStatus] = useState<GatedShellStatus | null>(null);
  const [generatingKey, setGeneratingKey] = useState(false);
  const [generatedKey, setGeneratedKey] = useState<string | null>(null);
  const [addingPattern, setAddingPattern] = useState(false);
  const [patternDraft, setPatternDraft] = useState<PatternDraft>({ name: '', pattern: '', format: '' });

  const loadStatus = useCallback(async () => {
    try {
      const s = await invoke<GatedShellStatus>('get_gated_shell_status');
      setStatus(s);
    } catch {
      // status is optional, ignore
    }
  }, []);

  useEffect(() => {
    loadStatus();
    const interval = setInterval(loadStatus, 30000);
    return () => clearInterval(interval);
  }, [loadStatus]);

  if (!config) return null;

  const gs = config.gated_shell;

  const update = (patch: Partial<typeof gs>) =>
    setConfig({ ...config, gated_shell: { ...gs, ...patch } });

  const updatePolicy = (patch: Partial<typeof gs.policy>) =>
    setConfig({ ...config, gated_shell: { ...gs, policy: { ...gs.policy, ...patch } } });

  const updateDiscovery = (patch: Partial<typeof gs.discovery>) =>
    setConfig({ ...config, gated_shell: { ...gs, discovery: { ...gs.discovery, ...patch } } });

  const updatePlugins = (patch: Partial<typeof gs.plugins>) =>
    setConfig({ ...config, gated_shell: { ...gs, plugins: { ...gs.plugins, ...patch } } });

  const updateTmux = (patch: Partial<typeof gs.tmux>) =>
    setConfig({ ...config, gated_shell: { ...gs, tmux: { ...gs.tmux, ...patch } } });

  const handleGenerateKey = async () => {
    setGeneratingKey(true);
    setGeneratedKey(null);
    try {
      const result = await invoke<{ private_key_hex: string; public_key_hex: string; note: string }>('generate_plugin_signing_key');
      setGeneratedKey(result.public_key_hex);
      notifySuccess('Plugin Key', 'Ed25519 keypair generated. Add the public key to trusted keys and keep the private key (signing.key) secure.');
    } catch (e) {
      notifyError('Plugin Key', `Failed to generate keypair: ${e}`);
    } finally {
      setGeneratingKey(false);
    }
  };

  const handleAddPattern = () => {
    const { name, pattern, format } = patternDraft;
    if (!name.trim() || !pattern.trim() || !format.trim()) return;
    try {
      new RegExp(pattern);
    } catch (e) {
      alert(`Invalid regex pattern: ${(e as Error).message}`);
      return;
    }
    updateDiscovery({
      extra_patterns: [
        ...gs.discovery.extra_patterns,
        { name: name.trim(), pattern: pattern.trim(), format: format.trim() },
      ],
    });
    setPatternDraft({ name: '', pattern: '', format: '' });
    setAddingPattern(false);
  };

  const handleRemovePattern = async (idx: number) => {
    if (!await showConfirm('Remove this custom discovery pattern?', { danger: true, confirmLabel: 'Remove' })) return;
    updateDiscovery({ extra_patterns: gs.discovery.extra_patterns.filter((_, i) => i !== idx) });
  };

  return (
    <div className="tab-content">
      {confirmModal}

      {/* ── Status Banner ─────────────────────────────────────────── */}
      {status && (
        <div className="settings-group" style={{ paddingBottom: 12 }}>
          <div style={{ display: 'flex', gap: 24, flexWrap: 'wrap', alignItems: 'center', fontSize: 13, color: 'var(--text-secondary)' }}>
            <span>
              Status:{' '}
              <strong style={{ color: status.enabled ? 'var(--success, #2ecc71)' : 'var(--text-secondary)' }}>
                {status.enabled ? 'Active' : 'Inactive'}
              </strong>
            </span>
            <span>Active sessions: <strong>{status.active_sessions}</strong></span>
            <span>Secrets masked: <strong>{status.secret_count}</strong></span>
            <button
              style={{ marginLeft: 'auto' }}
              onClick={() => invoke('open_shell_viewer').catch((e) => notifyError('NexiGate', `Failed to open viewer: ${e}`))}
            >
              Open Viewer
            </button>
          </div>
        </div>
      )}

      {/* ── General ───────────────────────────────────────────────── */}
      <div className="settings-group">
        <h3>
          NexiGate Shell{' '}
          <InfoTip text="Routes all shell tool commands through a secure PTY session with bidirectional secret filtering, policy enforcement, and full audit logging." />
        </h3>

        <div className="inline-toggle">
          <label className="toggle-label">
            <input type="checkbox" checked={gs.enabled} onChange={(e) => update({ enabled: e.target.checked })} />
            Enable NexiGate
          </label>
          <InfoTip text="When enabled, all shell commands run through the NexiGate filter. Secrets from Key Vault are automatically masked in both directions." />
        </div>

        <div className="inline-toggle">
          <label className="toggle-label">
            <input type="checkbox" checked={gs.debug_mode} onChange={(e) => update({ debug_mode: e.target.checked })} />
            Debug mode
          </label>
          <InfoTip text="Stores full raw PTY output in audit entries (may contain real secret values). Enable temporarily for troubleshooting only." />
        </div>

        <div className="inline-toggle">
          <label className="toggle-label">
            <input type="checkbox" checked={gs.record_sessions} onChange={(e) => update({ record_sessions: e.target.checked })} />
            Record sessions
          </label>
          <InfoTip text="Saves asciicast v2 .cast files to the recordings directory. Compatible with asciinema play and agg." />
        </div>

        {gs.record_sessions && (
          <label className="field">
            <span>
              Recordings directory{' '}
              <InfoTip text="Where .cast files are saved. Leave empty for the default location." />
            </span>
            <input
              type="text"
              value={gs.recordings_dir ?? ''}
              placeholder="Leave empty for default location"
              onChange={(e) => update({ recordings_dir: e.target.value || undefined })}
            />
          </label>
        )}

        <label className="field">
          <span>Shell binary <InfoTip text="The shell used for PTY sessions." /></span>
          <input type="text" value={gs.shell_binary} onChange={(e) => update({ shell_binary: e.target.value })} />
        </label>

        <label className="field">
          <span>Command timeout (seconds) <InfoTip text="Maximum time to wait for a command before returning a timeout error." /></span>
          <input
            type="number" min={1} max={3600}
            value={gs.command_timeout_secs}
            onChange={(e) => update({ command_timeout_secs: Number(e.target.value) })}
          />
        </label>

        <label className="field">
          <span>Max output bytes <InfoTip text="Output larger than this is truncated before being returned to the LLM." /></span>
          <input
            type="number" min={1024} max={10485760}
            value={gs.max_output_bytes}
            onChange={(e) => update({ max_output_bytes: Number(e.target.value) })}
          />
        </label>

        <label className="field">
          <span>Max audit entries <InfoTip text="Maximum audit log entries retained in memory per session." /></span>
          <input
            type="number" min={100} max={100000}
            value={gs.max_audit_entries}
            onChange={(e) => update({ max_audit_entries: Number(e.target.value) })}
          />
        </label>
      </div>

      {/* ── Policy ────────────────────────────────────────────────── */}
      <div className="settings-group">
        <h3>
          Policy{' '}
          <InfoTip text="Commands matching deny patterns are blocked before reaching the PTY. Built-in rules block rm -rf /, curl|sh pipes, disk writes, and filesystem formats." />
        </h3>

        <label className="field">
          <span>Max concurrent sessions <InfoTip text="Maximum simultaneous shell sessions across all agents." /></span>
          <input
            type="number" min={1} max={200}
            value={gs.policy.max_concurrent_sessions}
            onChange={(e) => updatePolicy({ max_concurrent_sessions: Number(e.target.value) })}
          />
        </label>

        <label className="field">
          <span>
            Additional deny patterns{' '}
            <InfoTip text="Extra regex patterns for commands to block (added on top of built-in rules). Press Enter or click Add after each pattern." />
          </span>
          <TagInput
            tags={gs.policy.deny_patterns}
            onChange={(tags) => updatePolicy({ deny_patterns: tags })}
            placeholder="e.g. sudo\s+rm or iptables.*-F"
          />
        </label>
      </div>

      {/* ── Dynamic Discovery ─────────────────────────────────────── */}
      <div className="settings-group">
        <h3>
          Dynamic Discovery{' '}
          <InfoTip text="Scans PTY output for secret patterns at runtime and auto-registers new secrets. Catches API keys read from .env files mid-session." />
        </h3>

        <div className="inline-toggle">
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={gs.discovery.enabled}
              onChange={(e) => updateDiscovery({ enabled: e.target.checked })}
            />
            Scan output for secrets
          </label>
          <InfoTip text="Regex scan of every command output. Detects Anthropic keys, GitHub tokens, JWTs, AWS keys, and 8 more formats automatically." />
        </div>

        <div className="inline-toggle">
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={gs.discovery.track_env_changes}
              onChange={(e) => updateDiscovery({ track_env_changes: e.target.checked })}
            />
            Track environment changes
          </label>
          <InfoTip text="Runs a side-channel printenv after each command to catch secrets exported via source .env or export KEY=value. Adds ~10ms per command." />
        </div>

        <label className="field">
          <span>Minimum secret length <InfoTip text="Values shorter than this are not auto-registered as secrets (reduces false positives)." /></span>
          <input
            type="number" min={8} max={200}
            value={gs.discovery.min_secret_length}
            onChange={(e) => updateDiscovery({ min_secret_length: Number(e.target.value) })}
          />
        </label>

        <div className="field">
          <span>
            Custom discovery patterns{' '}
            <InfoTip text="Additional regex patterns to scan for. Each match is auto-registered as a masked secret with a proxy token." />
          </span>

          {gs.discovery.extra_patterns.length > 0 && (
            <table style={{ width: '100%', borderCollapse: 'collapse', marginTop: 8, fontSize: 12 }}>
              <thead>
                <tr style={{ color: 'var(--text-secondary)', textAlign: 'left' }}>
                  <th style={{ padding: '4px 8px' }}>Name</th>
                  <th style={{ padding: '4px 8px' }}>Pattern</th>
                  <th style={{ padding: '4px 8px' }}>Format</th>
                  <th style={{ padding: '4px 8px' }}></th>
                </tr>
              </thead>
              <tbody>
                {gs.discovery.extra_patterns.map((p, i) => (
                  <tr key={i} style={{ borderTop: '1px solid var(--border)' }}>
                    <td style={{ padding: '4px 8px', fontFamily: 'monospace' }}>{p.name}</td>
                    <td style={{ padding: '4px 8px', fontFamily: 'monospace', maxWidth: 200, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{p.pattern}</td>
                    <td style={{ padding: '4px 8px' }}>{p.format}</td>
                    <td style={{ padding: '4px 8px' }}>
                      <button className="mcp-remove-btn" onClick={() => handleRemovePattern(i)}>Remove</button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}

          {addingPattern ? (
            <div style={{ display: 'flex', gap: 8, marginTop: 8, flexWrap: 'wrap', alignItems: 'flex-end' }}>
              <label style={{ display: 'flex', flexDirection: 'column', gap: 4, flex: '1 1 100px' }}>
                <span style={{ fontSize: 11, color: 'var(--text-secondary)' }}>Name</span>
                <input
                  type="text"
                  placeholder="my_service_key"
                  value={patternDraft.name}
                  onChange={(e) => setPatternDraft({ ...patternDraft, name: e.target.value })}
                />
              </label>
              <label style={{ display: 'flex', flexDirection: 'column', gap: 4, flex: '2 1 200px' }}>
                <span style={{ fontSize: 11, color: 'var(--text-secondary)' }}>Regex pattern</span>
                <input
                  type="text"
                  placeholder="msk-[A-Za-z0-9]{40}"
                  value={patternDraft.pattern}
                  style={{ fontFamily: 'monospace' }}
                  onChange={(e) => setPatternDraft({ ...patternDraft, pattern: e.target.value })}
                />
              </label>
              <label style={{ display: 'flex', flexDirection: 'column', gap: 4, flex: '1 1 100px' }}>
                <span style={{ fontSize: 11, color: 'var(--text-secondary)' }}>Format label</span>
                <input
                  type="text"
                  placeholder="myservice"
                  value={patternDraft.format}
                  onChange={(e) => setPatternDraft({ ...patternDraft, format: e.target.value })}
                />
              </label>
              <button onClick={handleAddPattern}>Add</button>
              <button
                onClick={() => {
                  setAddingPattern(false);
                  setPatternDraft({ name: '', pattern: '', format: '' });
                }}
              >
                Cancel
              </button>
            </div>
          ) : (
            <div className="action-buttons" style={{ marginTop: 8 }}>
              <button onClick={() => setAddingPattern(true)}>+ Add Pattern</button>
            </div>
          )}
        </div>
      </div>

      {/* ── Interactive Agent (Tmux Bridge) ───────────────────────── */}
      <div className="settings-group">
        <h3>
          Interactive Agent Bridge{' '}
          <InfoTip text="Lets the LLM start and control interactive programs (Claude Code, Aider, Python REPL, etc.) via tmux as a universal control layer." />
        </h3>

        <div className="inline-toggle">
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={gs.tmux?.enabled ?? false}
              onChange={(e) => updateTmux({ enabled: e.target.checked })}
            />
            Enable interactive agent tool
          </label>
          <InfoTip text="Exposes the nexibot_interactive_agent tool to the LLM. Requires tmux (macOS/Linux only)." />
        </div>

        {gs.tmux?.enabled && (
          <>
            <label className="field">
              <span>Poll interval (ms) <InfoTip text="How often to capture pane output while waiting for a state change." /></span>
              <input
                type="number" min={50} max={5000}
                value={gs.tmux.poll_interval_ms}
                onChange={(e) => updateTmux({ poll_interval_ms: Number(e.target.value) })}
              />
            </label>

            <label className="field">
              <span>Stability threshold (ms) <InfoTip text="How long pane content must be unchanged before returning UnknownStable to the LLM." /></span>
              <input
                type="number" min={200} max={30000}
                value={gs.tmux.content_stable_ms}
                onChange={(e) => updateTmux({ content_stable_ms: Number(e.target.value) })}
              />
            </label>

            <label className="field">
              <span>Wait timeout (ms) <InfoTip text="Maximum time to wait for a target state before giving up and returning Timeout." /></span>
              <input
                type="number" min={1000} max={600000}
                value={gs.tmux.wait_timeout_ms}
                onChange={(e) => updateTmux({ wait_timeout_ms: Number(e.target.value) })}
              />
            </label>

            <label className="field">
              <span>Max concurrent sessions <InfoTip text="Maximum number of simultaneous tmux interactive sessions." /></span>
              <input
                type="number" min={1} max={100}
                value={gs.tmux.max_sessions}
                onChange={(e) => updateTmux({ max_sessions: Number(e.target.value) })}
              />
            </label>
          </>
        )}
      </div>

      {/* ── Security Plugins ──────────────────────────────────────── */}
      <div className="settings-group">
        <h3>
          Security Plugins{' '}
          <InfoTip text="Ed25519-signed Rhai scripts that can observe and veto shell commands. Plugins run in a sandboxed interpreter with no filesystem or network access." />
        </h3>

        <div className="inline-toggle">
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={gs.plugins.enabled}
              onChange={(e) => updatePlugins({ enabled: e.target.checked })}
            />
            Enable plugins
          </label>
          <InfoTip text="Loads .rhai plugin files from the plugin directory. Each script must have a matching .manifest.json signed with a trusted Ed25519 key." />
        </div>

        {gs.plugins.enabled && (
          <>
            <label className="field">
              <span>
                Plugin directory{' '}
                <InfoTip text="Directory containing .rhai plugin files and their .manifest.json sidecar files." />
              </span>
              <input
                type="text"
                value={gs.plugins.plugin_dir ?? ''}
                placeholder="Leave empty for default location"
                onChange={(e) => updatePlugins({ plugin_dir: e.target.value || undefined })}
              />
            </label>

            <label className="field">
              <span>
                Trusted signing keys{' '}
                <InfoTip text="Ed25519 public keys (hex-encoded, 32 bytes) permitted to sign plugins. Generate a keypair with the button below." />
              </span>
              <TagInput
                tags={gs.plugins.trusted_keys}
                onChange={(keys) => updatePlugins({ trusted_keys: keys })}
                placeholder="Paste hex public key and press Enter"
              />
            </label>

            <div style={{ marginTop: 12, display: 'flex', flexDirection: 'column', gap: 8 }}>
              <div className="action-buttons">
                <button onClick={handleGenerateKey} disabled={generatingKey}>
                  {generatingKey ? 'Generating…' : 'Generate Signing Keypair'}
                </button>
              </div>
              {generatedKey && (
                <div style={{
                  background: 'var(--surface)',
                  border: '1px solid var(--border)',
                  borderRadius: 6,
                  padding: 12,
                  fontSize: 12,
                }}>
                  <p style={{ margin: '0 0 6px', color: 'var(--text-secondary)', fontWeight: 600 }}>
                    Public key — add this to Trusted signing keys above:
                  </p>
                  <code style={{ wordBreak: 'break-all', userSelect: 'all' }}>{generatedKey}</code>
                  <p style={{ margin: '8px 0 0', fontSize: 11, color: 'var(--warning, #f39c12)' }}>
                    The private key has been saved as <code>signing.key</code> in the plugin directory.
                    Keep it secure and never commit it to version control.
                  </p>
                </div>
              )}
            </div>
          </>
        )}
      </div>
    </div>
  );
}
