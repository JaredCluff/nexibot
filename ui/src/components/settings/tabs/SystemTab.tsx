import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useSettings } from '../SettingsContext';
import { InfoTip } from '../shared/InfoTip';
import { TagInput } from '../shared/TagInput';
import { SENSITIVE_PATH_SUGGESTIONS } from '../shared/suggestions';
import { notifyError, notifyInfo, notifySuccess } from '../../../shared/notify';
import { useConfirm } from '../../../shared/useConfirm';
import { UsageWidget } from '../../UsageWidget';

export function SystemTab() {
  const { confirm: showConfirm, modal: confirmModal } = useConfirm();
  const {
    config, setConfig, startupConfig, loadStartupConfig,
    bridgeStatus, setBridgeStatus,
    heartbeatConfig, setHeartbeatConfig, heartbeatRunning, setHeartbeatRunning,
    soulTemplates, currentSoul, setCurrentSoul,
    setSaveMessage,
    oauthProfiles, oauthStatus, loadOAuthData,
    subscriptions, loadSubscriptions,
    platformInfo,
  } = useSettings();

  const [appVersion, setAppVersion] = useState<string>('...');
  const [startingOAuth, setStartingOAuth] = useState(false);
  const [startingCliAuth, setStartingCliAuth] = useState(false);
  const [removingProfile, setRemovingProfile] = useState<string | null>(null);
  const [refreshingSubs, setRefreshingSubs] = useState(false);
  const [checkingSubId, setCheckingSubId] = useState<string | null>(null);
  const [bridgeOperating, setBridgeOperating] = useState(false);
  const [savingSoul, setSavingSoul] = useState(false);
  const [savingHeartbeat, setSavingHeartbeat] = useState(false);
  const [testingHeartbeat, setTestingHeartbeat] = useState(false);

  useEffect(() => {
    invoke<string>('get_app_version').then(setAppVersion).catch((e) => console.warn('Failed to get app version:', e));
  }, []);

  if (!config) return null;

  return (
    <div className="tab-content">
      {confirmModal}

      {/* Credit usage widget — shown when signed into Knowledge Nexus */}
      {config.k2k?.kn_auth_token && (
        <div className="settings-group">
          <h3>Credit Usage <InfoTip text="Your Knowledge Nexus credit balance and per-operation breakdown for the current billing period." /></h3>
          <UsageWidget
            knBaseUrl={config.k2k.kn_base_url}
            authToken={config.k2k.kn_auth_token}
          />
        </div>
      )}

      <div className="settings-group">
        <h3>Startup <InfoTip text="Control which services launch automatically when you log in." /></h3>
        <div className="inline-toggle">
          <label className="toggle-label">
            <input type="checkbox" checked={startupConfig.nexibot_at_login}
              onChange={async (e) => {
                try {
                  await invoke('set_nexibot_autostart', { enabled: e.target.checked });
                  loadStartupConfig();
                } catch (err) {
                  notifyError('Autostart', `Failed to update NexiBot autostart: ${err}`);
                }
              }} />
            Launch NexiBot at login
          </label>
          <InfoTip text="Start NexiBot automatically when you log in." />
        </div>
        <div className="inline-toggle">
          <label className="toggle-label">
            <input type="checkbox" checked={startupConfig.k2k_agent_at_login}
              onChange={async (e) => {
                try {
                  await invoke('set_k2k_agent_autostart', { enabled: e.target.checked });
                  loadStartupConfig();
                } catch (err) {
                  notifyError('Autostart', `Failed to update K2K Agent autostart: ${err}`);
                }
              }} />
            Launch K2K Agent at login
          </label>
          <InfoTip text="Start the Knowledge Nexus agent (`kn-agent`) at login for background indexing and federation." />
        </div>
        <label className="field">
          <span>K2K Agent Binary Path <InfoTip text="Path to the `kn-agent` binary. Leave empty for platform default." /></span>
          <input type="text" value={config.startup.k2k_agent_binary}
            onChange={(e) => setConfig({...config, startup: {...config.startup, k2k_agent_binary: e.target.value}})} />
        </label>
      </div>

      <div className="settings-group">
        <h3>Updates <InfoTip text="Check for and install NexiBot updates from GitHub Releases." /></h3>
        <p className="group-description">
          Current version: <strong>{appVersion}</strong>
        </p>
        <UpdateSection />
      </div>

      <div className="settings-group">
        <h3>Authentication<InfoTip text="Manage OAuth profiles and API keys for model access. OAuth is recommended; API keys are a fallback option." /></h3>

        {/* OAuth Status */}
        {oauthStatus && (
          <div className="status-indicator">
            <span className={`status-dot ${oauthStatus.is_expiring ? 'unhealthy' : 'healthy'}`} />
            <span>
              {oauthStatus.provider}: {oauthStatus.profile_name}
              {oauthStatus.is_expiring && ' (expiring soon)'}
              {oauthStatus.expires_at && <span className="hint"> — expires {new Date(oauthStatus.expires_at).toLocaleDateString()}</span>}
            </span>
          </div>
        )}
        {!oauthStatus && oauthProfiles.length === 0 && (
          <div className="info-text">No OAuth profiles configured. Sign in or use an API key below.</div>
        )}

        {/* OAuth Profiles */}
        {oauthProfiles.map((profile) => (
          <div key={`${profile.provider}-${profile.profile_name}`} className="mcp-server-card">
            <div className="mcp-server-header">
              <span className="mcp-server-name">{profile.provider}</span>
              <span className="mcp-server-command">{profile.profile_name}</span>
              {profile.expires_at && (
                <span className="mcp-tool-count">Expires: {new Date(profile.expires_at).toLocaleDateString()}</span>
              )}
              <button
                className="mcp-remove-btn"
                disabled={removingProfile === profile.profile_name}
                onClick={async () => {
                  if (!await showConfirm(`Remove OAuth profile "${profile.profile_name}"?`, { danger: true, confirmLabel: 'Remove' })) return;
                  setRemovingProfile(profile.profile_name);
                  try {
                    await invoke('remove_oauth_profile', { provider: profile.provider, profileName: profile.profile_name });
                    await loadOAuthData();
                  } catch (error) {
                    notifyError('OAuth', `Failed to remove profile: ${error}`);
                  } finally {
                    setRemovingProfile(null);
                  }
                }}
              >
                {removingProfile === profile.profile_name ? 'Removing...' : 'Remove'}
              </button>
            </div>
          </div>
        ))}

        {/* Auth Actions */}
        <div className="action-buttons">
          <button className="primary" disabled={startingOAuth} onClick={async () => {
            setStartingOAuth(true);
            try {
              await invoke('start_oauth_flow', { provider: 'anthropic' });
              await loadOAuthData();
            } catch (error) {
              notifyError('OAuth', `OAuth flow failed: ${error}`);
            } finally {
              setStartingOAuth(false);
            }
          }}>
            {startingOAuth ? 'Loading...' : 'Sign in with Anthropic'}
          </button>
          <button disabled={startingCliAuth} onClick={async () => {
            setStartingCliAuth(true);
            try {
              await invoke('start_claude_cli_auth');
              await loadOAuthData();
            } catch (error) {
              notifyError('Auth', `CLI auth failed: ${error}`);
            } finally {
              setStartingCliAuth(false);
            }
          }}>
            {startingCliAuth ? 'Loading...' : 'Use Claude CLI Auth'}
          </button>
        </div>

        {/* Fallback API Key */}
        <label className="field">
          <span>Claude API Key (fallback)<InfoTip text="Your Anthropic API key for direct access. Only needed if you're not using OAuth authentication." /></span>
          <input
            type="password"
            value={config.claude.api_key || ''}
            onChange={(e) => setConfig({ ...config, claude: { ...config.claude, api_key: e.target.value || undefined } })}
            placeholder="sk-ant-..."
          />
        </label>
      </div>

      {/* Subscriptions */}
      <div className="settings-group">
        <h3>Subscriptions<InfoTip text="Manage your active subscriptions to AI providers." /></h3>
        <div className="action-buttons">
          <button onClick={async () => {
            setRefreshingSubs(true);
            try {
              await invoke('refresh_subscriptions');
              await loadSubscriptions();
            } catch (error) {
              notifyError('Subscriptions', `Refresh failed: ${error}`);
            } finally {
              setRefreshingSubs(false);
            }
          }} disabled={refreshingSubs}>
            {refreshingSubs ? 'Loading...' : 'Refresh'}
          </button>
          <button onClick={async () => {
            try {
              await invoke('open_subscription_portal');
            } catch (error) {
              notifyError('Subscriptions', `Failed to open portal: ${error}`);
            }
          }}>
            Manage
          </button>
        </div>

        {subscriptions.length === 0 && (
          <div className="info-text">No active subscriptions.</div>
        )}

        {subscriptions.map((sub, i) => (
          <div key={i} className="mcp-server-card">
            <div className="mcp-server-header">
              <span className={`status-dot ${sub.status === 'active' ? 'healthy' : sub.status === 'expired' ? 'unhealthy' : 'inactive'}`} />
              <span className="mcp-server-name">{sub.provider}</span>
              <span className="mcp-server-command">{sub.tier}</span>
              {sub.expires_at && (
                <span className="mcp-tool-count">Expires: {new Date(sub.expires_at).toLocaleDateString()}</span>
              )}
            </div>
            <div className="action-buttons">
              <button disabled={checkingSubId === sub.provider} onClick={async () => {
                setCheckingSubId(sub.provider);
                try {
                  await invoke('check_subscription', { provider: sub.provider });
                  await loadSubscriptions();
                } catch (error) {
                  notifyError('Subscriptions', `Check failed: ${error}`);
                } finally {
                  setCheckingSubId(null);
                }
              }}>
                {checkingSubId === sub.provider ? 'Checking...' : 'Check Status'}
              </button>
              <button onClick={async () => {
                try {
                  await invoke('open_subscription_portal');
                } catch (error) {
                  notifyError('Subscriptions', `Failed to open portal: ${error}`);
                }
              }}>Manage</button>
            </div>
          </div>
        ))}
      </div>

      <div className="settings-group">
        <h3>Audio Input<InfoTip text="Low-level audio capture settings. Usually don't need to be changed." /></h3>
        <div className="settings-row">
          <label className="field">
            <span>Sample Rate<InfoTip text="Audio sample rate in Hz. 16000 Hz is standard for speech recognition." /></span>
            <input type="number" value={config.audio.sample_rate} readOnly />
          </label>
          <label className="field">
            <span>Channels<InfoTip text="Number of audio channels. 1 (mono) is standard for speech recognition." /></span>
            <input type="number" value={config.audio.channels} readOnly />
          </label>
        </div>
      </div>

      <div className="settings-group">
        <h3>Soul / Personality<InfoTip text="The soul defines NexiBot's personality, speaking style, and behavioral guidelines." /></h3>
        <p className="group-description">
          The soul defines NexiBot's personality, tone, and behavior. Choose a template or customize directly.
        </p>
        {soulTemplates.length > 0 && (
          <label className="field">
            <span>Soul Template<InfoTip text="Pre-made personality templates to quickly customize NexiBot's behavior." /></span>
            <select
              value=""
              onChange={async (e) => {
                if (e.target.value) {
                  try {
                    const soul = await invoke<{ content: string }>('load_soul_template', { templateName: e.target.value });
                    setCurrentSoul(soul.content || '');
                  } catch (error) {
                    notifyError('Soul', `Failed to load template: ${error}`);
                  }
                }
              }}
            >
              <option value="">Select a template...</option>
              {soulTemplates.map((t) => (
                <option key={t.name} value={t.name}>{t.name} — {t.description}</option>
              ))}
            </select>
          </label>
        )}
        <label className="field">
          <span>Soul Content<InfoTip text="The full personality definition. This text is included in every conversation to shape how NexiBot responds." /></span>
          <textarea
            value={currentSoul}
            onChange={(e) => setCurrentSoul(e.target.value)}
            rows={6}
            placeholder="Define NexiBot's personality and behavior..."
          />
        </label>
        <div className="action-buttons">
          <button className="primary" disabled={savingSoul} onClick={async () => {
            setSavingSoul(true);
            try {
              await invoke('update_soul', { newContent: currentSoul });
              setSaveMessage('Soul saved');
              setTimeout(() => setSaveMessage(''), 3000);
            } catch (error) {
              notifyError('Soul', `Failed to save soul: ${error}`);
            } finally {
              setSavingSoul(false);
            }
          }}>{savingSoul ? 'Saving…' : 'Save Soul'}</button>
        </div>
      </div>

      <div className="settings-group">
        <h3>Bridge Service<InfoTip text="The Anthropic Bridge translates between NexiBot and the Claude API. It must be running for AI features to work." /></h3>
        <p className="group-description">
          The Anthropic Bridge translates between NexiBot and the Claude API.
        </p>
        <div className="status-indicator">
          <span className={`status-dot ${bridgeStatus === 'Running' ? 'healthy' : bridgeStatus === 'Stopped' ? 'inactive' : 'unhealthy'}`} />
          <span>{bridgeStatus || 'Unknown'}</span>
        </div>
        <div className="action-buttons">
          <button disabled={bridgeOperating} onClick={async () => {
            setBridgeOperating(true);
            try { await invoke('start_bridge'); setBridgeStatus('Running'); } catch (e) { notifyError('Bridge', `Failed to start: ${e}`); } finally { setBridgeOperating(false); }
          }}>{bridgeOperating ? 'Working…' : 'Start'}</button>
          <button disabled={bridgeOperating} onClick={async () => {
            setBridgeOperating(true);
            try { await invoke('stop_bridge'); setBridgeStatus('Stopped'); } catch (e) { notifyError('Bridge', `Failed to stop: ${e}`); } finally { setBridgeOperating(false); }
          }}>Stop</button>
          <button disabled={bridgeOperating} onClick={async () => {
            setBridgeOperating(true);
            try { await invoke('restart_bridge'); setBridgeStatus('Running'); } catch (e) { notifyError('Bridge', `Failed to restart: ${e}`); } finally { setBridgeOperating(false); }
          }}>Restart</button>
        </div>
      </div>

      <div className="settings-group">
        <h3>Heartbeat<InfoTip text="Periodic health checks that monitor whether the bridge service and other components are responsive." /></h3>
        <p className="group-description">
          Periodic health checks to monitor service availability.
        </p>
        <div className="status-indicator">
          <span className={`status-dot ${heartbeatRunning ? 'healthy' : 'inactive'}`} />
          <span>{heartbeatRunning ? 'Running' : 'Stopped'}</span>
        </div>
        {heartbeatConfig && (
          <>
            <div className="inline-toggle">
              <label className="toggle-label">
                <input
                  type="checkbox"
                  checked={heartbeatConfig.enabled}
                  onChange={(e) => setHeartbeatConfig({ ...heartbeatConfig, enabled: e.target.checked })}
                />
                Enable Heartbeat<InfoTip text="Run automatic health checks at regular intervals." />
              </label>
            </div>
            <label className="field">
              <span>Interval (seconds)<InfoTip text="How often to run health checks, in seconds." /></span>
              <input
                type="number"
                min="5" max="3600"
                value={heartbeatConfig.interval_seconds}
                onChange={(e) => setHeartbeatConfig({ ...heartbeatConfig, interval_seconds: parseInt(e.target.value) || 60 })}
              />
            </label>
            <div className="action-buttons">
              <button className="primary" disabled={savingHeartbeat} onClick={async () => {
                setSavingHeartbeat(true);
                try {
                  await invoke('update_heartbeat_config', { config: heartbeatConfig });
                  if (heartbeatConfig.enabled && !heartbeatRunning) {
                    await invoke('start_heartbeat');
                    setHeartbeatRunning(true);
                  } else if (!heartbeatConfig.enabled && heartbeatRunning) {
                    await invoke('stop_heartbeat');
                    setHeartbeatRunning(false);
                  }
                  setSaveMessage('Heartbeat config saved');
                  setTimeout(() => setSaveMessage(''), 3000);
                } catch (error) {
                  notifyError('Heartbeat', `Failed to update heartbeat: ${error}`);
                } finally {
                  setSavingHeartbeat(false);
                }
              }}>{savingHeartbeat ? 'Applying…' : 'Apply'}</button>
              <button disabled={testingHeartbeat} onClick={async () => {
                setTestingHeartbeat(true);
                try {
                  const result = await invoke<{ status: string }>('trigger_heartbeat');
                  notifyInfo('Heartbeat', `Result: ${JSON.stringify(result)}`);
                } catch (error) {
                  notifyError('Heartbeat', `Heartbeat failed: ${error}`);
                } finally {
                  setTestingHeartbeat(false);
                }
              }}>{testingHeartbeat ? 'Testing…' : 'Test Now'}</button>
            </div>
          </>
        )}
      </div>
      {/* WebSocket Gateway */}
      <div className="settings-group">
        <h3>WebSocket Gateway<InfoTip text="Multi-user server mode. Enables multiple clients to connect to NexiBot simultaneously via WebSocket." /></h3>
        <p className="group-description">
          The gateway allows external clients to connect to NexiBot via WebSocket.
        </p>
        <div className="inline-toggle">
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={config.gateway?.enabled ?? false}
              onChange={(e) => setConfig({ ...config, gateway: { ...(config.gateway ?? { enabled: false, port: 18792, bind_address: '127.0.0.1', auth_mode: 'Token' as const, max_connections: 50 }), enabled: e.target.checked } })}
            />
            Enable Gateway<InfoTip text="Start the WebSocket gateway server for multi-user access." />
          </label>
        </div>
        <label className="field">
          <span>Port<InfoTip text="TCP port the gateway listens on. Default: 18792." /></span>
          <input
            type="number"
            min={1024} max={65535}
            value={config.gateway?.port ?? 18792}
            onChange={(e) => setConfig({ ...config, gateway: { ...config.gateway, port: parseInt(e.target.value) || 18792 } })}
          />
        </label>
        <label className="field">
          <span>Bind Address<InfoTip text="Network interface to bind to. '127.0.0.1' = loopback only (secure default). '0.0.0.0' = all interfaces (requires TLS for remote access)." /></span>
          <select
            value={config.gateway?.bind_address ?? '127.0.0.1'}
            onChange={(e) => setConfig({ ...config, gateway: { ...config.gateway, bind_address: e.target.value } })}
          >
            <option value="127.0.0.1">127.0.0.1 (loopback only — recommended)</option>
            <option value="0.0.0.0">0.0.0.0 (all interfaces — requires TLS)</option>
          </select>
        </label>
        {config.gateway?.bind_address === '0.0.0.0' && (
          <div className="warning-banner">Binding to all interfaces exposes the gateway to the network. Ensure TLS is configured for remote access.</div>
        )}
        <label className="field">
          <span>Auth Mode<InfoTip text="Token: pre-shared bearer tokens. Password: SHA-256 hash comparison. Open: no auth (development only)." /></span>
          <select
            value={config.gateway?.auth_mode ?? 'Token'}
            onChange={(e) => setConfig({ ...config, gateway: { ...config.gateway, auth_mode: e.target.value as 'Token' | 'Password' | 'Open' } })}
          >
            <option value="Token">Token (pre-shared bearer tokens)</option>
            <option value="Password">Password (hash comparison)</option>
            <option value="Open">Open (development only)</option>
          </select>
        </label>
        {config.gateway?.auth_mode === 'Open' && (
          <div className="warning-banner">Open auth mode allows unauthenticated access. Only use for local development.</div>
        )}
        <label className="field">
          <span>Max Connections<InfoTip text="Maximum number of simultaneous WebSocket connections. Default: 50." /></span>
          <input
            type="number"
            min={1} max={10000}
            value={config.gateway?.max_connections ?? 50}
            onChange={(e) => setConfig({ ...config, gateway: { ...config.gateway, max_connections: parseInt(e.target.value) || 50 } })}
          />
        </label>
      </div>

      {/* Docker Sandbox */}
      <div className="settings-group">
        <h3>Docker Sandbox<InfoTip text="Isolate command execution in Docker containers for security. Prevents direct access to the host filesystem." /></h3>
        <p className="group-description">
          Run commands in isolated Docker containers. Requires Docker to be installed and running.
        </p>
        <div className="inline-toggle">
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={config.sandbox?.enabled ?? true}
              onChange={(e) => setConfig({ ...config, sandbox: { ...(config.sandbox ?? { enabled: true, image: 'debian:bookworm-slim', non_root_user: 'sandbox', memory_limit: '512m', cpu_limit: 1.0, network_mode: 'none', timeout_seconds: 60, blocked_paths: [] }), enabled: e.target.checked } })}
            />
            Enable Sandbox<InfoTip text="Run all command execution inside Docker containers." />
          </label>
        </div>
        <label className="field">
          <span>Docker Image<InfoTip text="Docker image used for sandbox containers." /></span>
          <input
            type="text"
            value={config.sandbox?.image ?? 'debian:bookworm-slim'}
            onChange={(e) => setConfig({ ...config, sandbox: { ...config.sandbox, image: e.target.value } })}
            placeholder="debian:bookworm-slim"
          />
        </label>
        <label className="field">
          <span>Non-Root User<InfoTip text="Username for the non-root user inside the container." /></span>
          <input
            type="text"
            value={config.sandbox?.non_root_user ?? 'sandbox'}
            onChange={(e) => setConfig({ ...config, sandbox: { ...config.sandbox, non_root_user: e.target.value } })}
            placeholder="sandbox"
          />
        </label>
        <div className="settings-row">
          <label className="field">
            <span>Memory Limit<InfoTip text="Maximum memory allocation for containers (e.g., 512m, 1g)." /></span>
            <input
              type="text"
              value={config.sandbox?.memory_limit ?? '512m'}
              onChange={(e) => setConfig({ ...config, sandbox: { ...config.sandbox, memory_limit: e.target.value } })}
              placeholder="512m"
            />
          </label>
          <label className="field">
            <span>CPU Limit<InfoTip text="Number of CPU cores allocated to the container. Default: 1.0." /></span>
            <input
              type="number"
              min={0.1} max={16} step={0.1}
              value={config.sandbox?.cpu_limit ?? 1.0}
              onChange={(e) => setConfig({ ...config, sandbox: { ...config.sandbox, cpu_limit: parseFloat(e.target.value) || 1.0 } })}
            />
          </label>
        </div>
        <div className="settings-row">
          <label className="field">
            <span>Network Mode<InfoTip text="Docker network mode. 'none' disables networking (most secure). 'bridge' allows outbound access." /></span>
            <select
              value={config.sandbox?.network_mode ?? 'none'}
              onChange={(e) => setConfig({ ...config, sandbox: { ...config.sandbox, network_mode: e.target.value } })}
            >
              <option value="none">none (no network access)</option>
              <option value="bridge">bridge (outbound access)</option>
            </select>
          </label>
          <label className="field">
            <span>Timeout (seconds)<InfoTip text="Maximum time a sandboxed command can run before being killed." /></span>
            <input
              type="number"
              min={1} max={3600}
              value={config.sandbox?.timeout_seconds ?? 60}
              onChange={(e) => setConfig({ ...config, sandbox: { ...config.sandbox, timeout_seconds: parseInt(e.target.value) || 60 } })}
            />
          </label>
        </div>
        <label className="field">
          <span>Blocked Paths<InfoTip text="Host paths that cannot be mounted into sandbox containers." /></span>
        </label>
        <TagInput
          tags={config.sandbox?.blocked_paths ?? []}
          onChange={(tags) => setConfig({ ...config, sandbox: { ...config.sandbox, blocked_paths: tags } })}
          placeholder="/etc, /proc, /sys..."
          suggestions={SENSITIVE_PATH_SUGGESTIONS}
        />
        <label className="field">
          <span>Seccomp Profile (optional)<InfoTip text="Path to a custom seccomp profile for syscall filtering." /></span>
          <input
            type="text"
            value={config.sandbox?.seccomp_profile ?? ''}
            onChange={(e) => setConfig({ ...config, sandbox: { ...config.sandbox, seccomp_profile: e.target.value || undefined } })}
            placeholder="Leave empty for Docker default"
          />
        </label>
        <label className="field">
          <span>AppArmor Profile (optional)<InfoTip text="Name of an AppArmor profile for mandatory access control." /></span>
          <input
            type="text"
            value={config.sandbox?.apparmor_profile ?? ''}
            onChange={(e) => setConfig({ ...config, sandbox: { ...config.sandbox, apparmor_profile: e.target.value || undefined } })}
            placeholder="Leave empty for Docker default"
          />
        </label>
      </div>

      {/* Background Tasks */}
      <BackgroundTasksSection />
    </div>
  );
}

// ─── Background Tasks sub-component ──────────────────────────────────────────

interface BackgroundTask {
  id: string;
  description: string;
  status: string;
  created_at: string;
  updated_at: string;
  progress: string | null;
  result_summary: string | null;
}

function BackgroundTasksSection() {
  const [tasks, setTasks] = useState<BackgroundTask[]>([]);
  const [loading, setLoading] = useState(false);

  const loadTasks = async () => {
    setLoading(true);
    try {
      const list = await invoke<BackgroundTask[]>('list_background_tasks');
      setTasks(list);
    } catch { /* not critical */ }
    finally { setLoading(false); }
  };

  const statusColor = (status: string) => {
    if (status === 'Running') return 'var(--success)';
    if (status === 'Completed') return 'var(--primary)';
    if (status === 'Failed') return 'var(--error)';
    if (status === 'Cancelled') return 'var(--text-secondary)';
    return 'var(--text-secondary)';
  };

  return (
    <div className="settings-group">
      <h3>Background Tasks<InfoTip text="View currently running and recently completed background tasks." /></h3>
      <div className="action-buttons">
        <button onClick={loadTasks} disabled={loading}>
          {loading ? 'Loading...' : 'Refresh'}
        </button>
      </div>

      {tasks.length === 0 && !loading && (
        <div className="info-text">No background tasks. Click Refresh to check.</div>
      )}

      {tasks.map((task) => (
        <div key={task.id} className="mcp-server-card">
          <div className="mcp-server-header">
            <span className="mcp-status-dot" style={{ backgroundColor: statusColor(task.status) }} />
            <span className="mcp-server-name">{task.description}</span>
            <span className="mcp-server-command">{task.status}</span>
            <span className="mcp-tool-count">{new Date(task.created_at).toLocaleString()}</span>
          </div>
          {task.progress && (
            <div style={{ fontSize: '12px', color: 'var(--text-secondary)', margin: '4px 0' }}>
              Progress: {task.progress}
            </div>
          )}
          {task.result_summary && (
            <div style={{ fontSize: '12px', color: 'var(--text-secondary)', margin: '4px 0', whiteSpace: 'pre-wrap' }}>
              {task.result_summary}
            </div>
          )}
        </div>
      ))}
    </div>
  );
}

// ─── Update sub-component ────────────────────────────────────────────────────

function UpdateSection() {
  const [updateInfo, setUpdateInfo] = useState<{ version: string; date: string | null; body: string | null } | null>(null);
  const [checkingUpdate, setCheckingUpdate] = useState(false);
  const [installingUpdate, setInstallingUpdate] = useState(false);
  const { setSaveMessage } = useSettings();

  return (
    <>
      <div className="action-buttons">
        <button
          className="primary"
          disabled={checkingUpdate}
          onClick={async () => {
            setCheckingUpdate(true);
            setUpdateInfo(null);
            try {
              const info = await invoke<typeof updateInfo>('check_for_updates');
              setUpdateInfo(info);
              if (!info) {
                setSaveMessage('You are up to date!');
                setTimeout(() => setSaveMessage(''), 3000);
              }
            } catch (error) {
              notifyError('Updates', `Failed to check for updates: ${error}`);
            } finally {
              setCheckingUpdate(false);
            }
          }}
        >
          {checkingUpdate ? 'Checking...' : 'Check for Updates'}
        </button>
      </div>
      {updateInfo && (
        <div style={{ margin: '8px 0' }}>
          <div className="info-text">
            <strong>Version {updateInfo.version}</strong> is available
            {updateInfo.date && <> (released {updateInfo.date})</>}
          </div>
          {updateInfo.body && (
            <div style={{ fontSize: '12px', color: 'var(--text-secondary)', margin: '4px 0', whiteSpace: 'pre-wrap' }}>
              {updateInfo.body}
            </div>
          )}
          <button
            className="primary"
            disabled={installingUpdate}
            onClick={async () => {
              setInstallingUpdate(true);
              try {
                await invoke('install_update');
                notifySuccess('Updates', 'Update installed. NexiBot will restart.');
              } catch (error) {
                notifyError('Updates', `Failed to install update: ${error}`);
              } finally {
                setInstallingUpdate(false);
              }
            }}
          >
            {installingUpdate ? 'Installing...' : 'Install Update'}
          </button>
        </div>
      )}
    </>
  );
}
