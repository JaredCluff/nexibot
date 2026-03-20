import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useSettings } from '../SettingsContext';
import { InfoTip } from '../shared/InfoTip';
import type { MCPPreset } from '../SettingsContext';
import { notifyError, notifySuccess } from '../../../shared/notify';
import { useConfirm } from '../../../shared/useConfirm';

const MCP_PRESETS: MCPPreset[] = [
  {
    name: 'playwright',
    description: 'Browser control and automation via Playwright',
    command: 'npx',
    args: ['-y', '@playwright/mcp@latest'],
    env: {},
  },
  {
    name: 'filesystem',
    description: 'File system access (read/write files in a temp directory)',
    command: 'npx',
    args: ['-y', '@modelcontextprotocol/server-filesystem', '.'],
    env: {},
  },
];

const getStatusColor = (status: string | { Error: string }) => {
  if (status === 'Connected') return 'var(--success)';
  if (status === 'Connecting') return 'var(--warning)';
  if (typeof status === 'object' && 'Error' in status) return 'var(--error)';
  return 'var(--text-secondary)';
};

interface MCPToolItem {
  name: string;
  prefixed_name: string;
  description: string;
  server_name: string;
}

export function ToolsTab() {
  const { config, setConfig, mcpServers, loadMcpServers, accessibilityPermissions, checkAccessibility, platformInfo, bridgeStatus } = useSettings();
  const { confirm: showConfirm, modal: confirmModal } = useConfirm();

  // Lazy-load accessibility permission status only when this tab is opened,
  // not at app startup, to avoid the macOS "control System Events" TCC prompt
  // appearing before the user has intentionally navigated to Computer Use settings.
  useEffect(() => {
    checkAccessibility();
  }, [checkAccessibility]);

  const [showAddServer, setShowAddServer] = useState(false);
  const [newServer, setNewServer] = useState({ name: '', command: '', args: '', env: '' });

  // All discovered tools state
  const [allTools, setAllTools] = useState<MCPToolItem[]>([]);
  const [loadingTools, setLoadingTools] = useState(false);

  // Bridge install state
  const [installingBridge, setInstallingBridge] = useState(false);

  if (!config) return null;

  const existingNames = mcpServers.map(s => s.name);
  const availablePresets = MCP_PRESETS.filter(p => !existingNames.includes(p.name));

  const handleAddPreset = async (preset: MCPPreset) => {
    try {
      await invoke('add_mcp_server', {
        config: {
          name: preset.name,
          enabled: true,
          command: preset.command,
          args: preset.args,
          env: preset.env,
        },
      });
      loadMcpServers();
    } catch (error) {
      notifyError('MCP', `Failed to add preset: ${error}`);
    }
  };

  const handleAddMcpServer = async () => {
    if (!newServer.name || !newServer.command) return;
    try {
      const args = newServer.args ? newServer.args.split(',').map(a => a.trim()).filter(Boolean) : [];
      const env: Record<string, string> = {};
      if (newServer.env) {
        newServer.env.split(',').forEach(pair => {
          const [key, ...vals] = pair.split('=');
          if (key && vals.length) env[key.trim()] = vals.join('=').trim();
        });
      }
      await invoke('add_mcp_server', {
        config: { name: newServer.name, enabled: true, command: newServer.command, args, env },
      });
      setNewServer({ name: '', command: '', args: '', env: '' });
      setShowAddServer(false);
      loadMcpServers();
    } catch (error) {
      notifyError('MCP', `Failed to add server: ${error}`);
    }
  };

  const handleRemoveMcpServer = async (name: string) => {
    if (!await showConfirm(`Remove MCP server "${name}"?`, { danger: true })) return;
    try {
      await invoke('remove_mcp_server', { name });
      loadMcpServers();
    } catch (error) {
      notifyError('MCP', `Failed to remove server: ${error}`);
    }
  };

  const handleToggleMcpServer = async (name: string, connected: boolean) => {
    try {
      if (connected) {
        await invoke('disconnect_mcp_server', { name });
      } else {
        await invoke('connect_mcp_server', { name });
      }
      loadMcpServers();
    } catch (error) {
      notifyError('MCP', `Failed to ${connected ? 'disconnect' : 'connect'} server: ${error}`);
    }
  };

  return (
    <div className="tab-content">
      {confirmModal}
      {/* Web Search Providers */}
      <div className="settings-group">
        <h3>Web Search <InfoTip text="Configure search providers for the built-in nexibot_web_search tool. Providers are tried in priority order. Brave and Tavily require API keys. DuckDuckGo (browser) is zero-config but less reliable." /></h3>
        <p className="group-description">
          Active provider order: {(config.search?.search_priority || ['brave', 'tavily', 'browser']).join(' \u2192 ')}
        </p>
        <label className="field">
          <span>Brave Search API Key <InfoTip text="Free tier: 2,000 queries/month. Get a key at api.search.brave.com" /></span>
          <input
            type="password"
            value={config.search?.brave_api_key || ''}
            placeholder="BSA..."
            onChange={(e) => setConfig({...config, search: {...(config.search || { search_priority: ['brave', 'tavily', 'browser'], default_result_count: 5 }), brave_api_key: e.target.value || undefined}})}
          />
        </label>
        <label className="field">
          <span>Tavily API Key <InfoTip text="Free tier: 1,000 queries/month. Get a key at tavily.com" /></span>
          <input
            type="password"
            value={config.search?.tavily_api_key || ''}
            placeholder="tvly-..."
            onChange={(e) => setConfig({...config, search: {...(config.search || { search_priority: ['brave', 'tavily', 'browser'], default_result_count: 5 }), tavily_api_key: e.target.value || undefined}})}
          />
        </label>
        <label className="field">
          <span>Default Results Count <InfoTip text="Number of search results to return per query (1-20)." /></span>
          <input
            type="number"
            min={1}
            max={20}
            value={config.search?.default_result_count || 5}
            onChange={(e) => setConfig({...config, search: {...(config.search || { search_priority: ['brave', 'tavily', 'browser'], default_result_count: 5 }), default_result_count: parseInt(e.target.value) || 5}})}
          />
        </label>
      </div>

      {/* Quick Add Presets */}
      {availablePresets.length > 0 && (
        <div className="settings-group">
          <h3>Quick Add <InfoTip text="Pre-configured MCP servers you can add with one click. These are popular community servers that provide useful capabilities." /></h3>
          <p className="group-description">One-click add popular MCP servers.</p>
          <div className="mcp-presets">
            {availablePresets.map((preset) => (
              <div key={preset.name} className="mcp-preset-card" onClick={() => handleAddPreset(preset)}>
                <div className="mcp-preset-info">
                  <span className="mcp-server-name">{preset.name}</span>
                  <span className="mcp-tool-desc">{preset.description}</span>
                </div>
                <button className="mcp-toggle-btn">+ Add</button>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* MCP Servers */}
      <div className="settings-group">
        <h3>MCP Servers <InfoTip text="MCP (Model Context Protocol) servers extend NexiBot with external tools like file access, web browsing, databases, and more." /></h3>
        <p className="group-description">
          Connect to MCP (Model Context Protocol) servers to give NexiBot additional capabilities like file access, web browsing, and more.
        </p>

        {mcpServers.length === 0 && !showAddServer && (
          <div className="info-text">No MCP servers configured. Add one to get started.</div>
        )}

        {mcpServers.map((server) => {
          const isConnected = server.status === 'Connected';
          return (
            <div key={server.name} className="mcp-server-card">
              <div className="mcp-server-header">
                <span className="mcp-status-dot" style={{ backgroundColor: getStatusColor(server.status) }} />
                <span className="mcp-server-name">{server.name}</span>
                <span className="mcp-server-command">{server.command}</span>
                <span className="mcp-tool-count">{server.tool_count} tools</span>
                <button
                  className={`mcp-toggle-btn ${isConnected ? 'connected' : ''}`}
                  onClick={() => handleToggleMcpServer(server.name, isConnected)}
                >
                  {isConnected ? 'Disconnect' : 'Connect'}
                </button>
                <button
                  className="mcp-remove-btn"
                  onClick={() => handleRemoveMcpServer(server.name)}
                >
                  Remove
                </button>
              </div>
              {isConnected && server.tools.length > 0 && (
                <div className="mcp-tools-list">
                  {server.tools.map((tool) => (
                    <div key={tool.prefixed_name} className="mcp-tool-item">
                      <span className="mcp-tool-name">{tool.name}</span>
                      <span className="mcp-tool-desc">{tool.description}</span>
                    </div>
                  ))}
                </div>
              )}
              {typeof server.status === 'object' && 'Error' in server.status && (
                <div className="mcp-error">{server.status.Error}</div>
              )}
            </div>
          );
        })}

        {showAddServer ? (
          <div className="mcp-add-form">
            <label className="field">
              <span>Server Name <InfoTip text="A friendly name for this server. Used to identify it in the list." /></span>
              <input
                type="text"
                placeholder="Server name (e.g., filesystem)"
                value={newServer.name}
                onChange={(e) => setNewServer({ ...newServer, name: e.target.value })}
              />
            </label>
            <label className="field">
              <span>Command <InfoTip text="The command to launch the MCP server process (e.g., npx, python, node)." /></span>
              <input
                type="text"
                placeholder="Command (e.g., npx)"
                value={newServer.command}
                onChange={(e) => setNewServer({ ...newServer, command: e.target.value })}
              />
            </label>
            <label className="field">
              <span>Args <InfoTip text="Command-line arguments, comma-separated. These are passed to the server command." /></span>
              <input
                type="text"
                placeholder="Args (comma-separated, e.g., -y, @modelcontextprotocol/server-filesystem, .)"
                value={newServer.args}
                onChange={(e) => setNewServer({ ...newServer, args: e.target.value })}
              />
            </label>
            <label className="field">
              <span>Env Vars <InfoTip text="Environment variables for the server process, as KEY=value pairs separated by commas." /></span>
              <input
                type="text"
                placeholder="Env vars (comma-separated, e.g., KEY=value, OTHER=val)"
                value={newServer.env}
                onChange={(e) => setNewServer({ ...newServer, env: e.target.value })}
              />
            </label>
            <div className="mcp-add-actions">
              <button onClick={handleAddMcpServer} disabled={!newServer.name || !newServer.command}>
                Add Server
              </button>
              <button onClick={() => setShowAddServer(false)}>Cancel</button>
            </div>
          </div>
        ) : (
          <button className="mcp-add-btn" onClick={() => setShowAddServer(true)}>
            + Add MCP Server
          </button>
        )}
      </div>

      {/* Computer Use */}
      <div className="settings-group">
        <h3>Computer Use <InfoTip text="Lets the AI control your desktop — take screenshots, move the mouse, click, type, and scroll. Requires accessibility permissions on macOS." /></h3>
        <p className="group-description">
          Enable Computer Use to let Claude control your desktop — take screenshots, move the mouse, click, type, and scroll.
        </p>
        <div className="inline-toggle">
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={config.computer_use.enabled}
              onChange={(e) => setConfig({
                ...config,
                computer_use: { ...config.computer_use, enabled: e.target.checked },
              })}
            />
            Enable Computer Use <InfoTip text="Grant the AI ability to see your screen and interact with your desktop. Use with caution." />
          </label>
        </div>
        {config.computer_use.enabled && (
          <>
            <div className="inline-toggle">
              <label className="toggle-label">
                <input
                  type="checkbox"
                  checked={config.computer_use.require_confirmation}
                  onChange={(e) => setConfig({
                    ...config,
                    computer_use: { ...config.computer_use, require_confirmation: e.target.checked },
                  })}
                />
                Require confirmation before actions <InfoTip text="When enabled, you must approve each action before the AI performs it. Recommended for safety." />
              </label>
            </div>
            {platformInfo?.os === 'macos' && (
              <div className="info-text">
                Accessibility: {accessibilityPermissions === true ? 'Granted' : accessibilityPermissions === false ? (
                  <>Not granted — <button className="mcp-toggle-btn" onClick={() => invoke('request_accessibility_permissions')}>Grant Access</button></>
                ) : 'Unknown'}
              </div>
            )}
          </>
        )}
      </div>

      {/* Browser CDP */}
      <div className="settings-group">
        <h3>Browser <InfoTip text="Browser automation via Chrome DevTools Protocol. The AI can navigate pages, click elements, fill forms, and take screenshots." /></h3>
        <p className="group-description">
          Enable browser automation to let Claude navigate web pages, extract content, click elements, and take screenshots via Chrome DevTools Protocol.
        </p>
        <div className="inline-toggle">
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={config.browser?.enabled ?? false}
              onChange={(e) => setConfig({
                ...config,
                browser: { ...(config.browser ?? { enabled: false, headless: true, default_timeout_ms: 30000, viewport_width: 1280, viewport_height: 720, require_confirmation: true, allowed_domains: [], use_guardrails: true }), enabled: e.target.checked },
              })}
            />
            Enable Browser Tool <InfoTip text="Allow the AI to open and control a Chrome browser window for web tasks." />
          </label>
        </div>
        {config.browser?.enabled && (
          <>
            <div className="inline-toggle">
              <label className="toggle-label">
                <input
                  type="checkbox"
                  checked={config.browser.require_confirmation ?? true}
                  onChange={(e) => setConfig({
                    ...config,
                    browser: { ...config.browser, require_confirmation: e.target.checked },
                  })}
                />
                Require confirmation before actions <InfoTip text="Ask for approval before each browser action." />
              </label>
            </div>
            <div className="inline-toggle">
              <label className="toggle-label">
                <input
                  type="checkbox"
                  checked={config.browser.use_guardrails ?? true}
                  onChange={(e) => setConfig({
                    ...config,
                    browser: { ...config.browser, use_guardrails: e.target.checked },
                  })}
                />
                Run through guardrails check <InfoTip text="Run browser actions through the guardrails safety pipeline before execution." />
              </label>
            </div>
            <label className="field">
              <span>Allowed Domains (one per line, empty = all) <InfoTip text="Restrict browsing to these domains only (one per line). Leave empty to allow all domains." /></span>
              <textarea
                rows={3}
                value={(config.browser.allowed_domains ?? []).join('\n')}
                placeholder={"example.com\ndocs.example.org"}
                onChange={(e) => setConfig({
                  ...config,
                  browser: {
                    ...config.browser,
                    allowed_domains: e.target.value
                      .split('\n')
                      .map((d) => d.trim())
                      .filter((d) => d.length > 0),
                  },
                })}
              />
            </label>
            <div className="inline-toggle">
              <label className="toggle-label">
                <input
                  type="checkbox"
                  checked={config.browser.headless}
                  onChange={(e) => setConfig({
                    ...config,
                    browser: { ...config.browser, headless: e.target.checked },
                  })}
                />
                Headless mode (no visible browser window) <InfoTip text="Run the browser without a visible window. Faster but you can't watch what's happening." />
              </label>
            </div>
            <label className="field">
              <span>Chrome Path (optional) <InfoTip text="Path to Chrome/Chromium executable. Leave empty to auto-detect." /></span>
              <input
                type="text"
                value={config.browser.chrome_path ?? ''}
                placeholder="Auto-detect"
                onChange={(e) => setConfig({
                  ...config,
                  browser: { ...config.browser, chrome_path: e.target.value || undefined },
                })}
              />
            </label>
            <label className="field">
              <span>Viewport Width <InfoTip text="Browser window dimensions in pixels. Affects screenshots and layout." /></span>
              <input
                type="number"
                value={config.browser.viewport_width}
                onChange={(e) => setConfig({
                  ...config,
                  browser: { ...config.browser, viewport_width: parseInt(e.target.value) || 1280 },
                })}
              />
            </label>
            <label className="field">
              <span>Viewport Height <InfoTip text="Browser window dimensions in pixels. Affects screenshots and layout." /></span>
              <input
                type="number"
                value={config.browser.viewport_height}
                onChange={(e) => setConfig({
                  ...config,
                  browser: { ...config.browser, viewport_height: parseInt(e.target.value) || 720 },
                })}
              />
            </label>
            <label className="field">
              <span>Default Timeout (ms) <InfoTip text="Maximum time (ms) to wait for page loads and element interactions before timing out." /></span>
              <input
                type="number"
                value={config.browser.default_timeout_ms}
                onChange={(e) => setConfig({
                  ...config,
                  browser: { ...config.browser, default_timeout_ms: parseInt(e.target.value) || 30000 },
                })}
              />
            </label>
          </>
        )}
      </div>

      {/* All Discovered Tools */}
      <div className="settings-group">
        <h3>All Discovered Tools <InfoTip text="List all tools discovered across all connected MCP servers. Useful for seeing the full set of capabilities available." /></h3>
        <div className="action-buttons">
          <button className="primary" onClick={async () => {
            setLoadingTools(true);
            try {
              const tools = await invoke<MCPToolItem[]>('list_mcp_tools');
              setAllTools(tools);
            } catch (error) {
              notifyError('MCP', `Failed to load tools: ${error}`);
            } finally {
              setLoadingTools(false);
            }
          }} disabled={loadingTools}>
            {loadingTools ? 'Loading...' : 'Load All Tools'}
          </button>
        </div>
        {allTools.length > 0 && (
          <>
            <div className="info-text" style={{ marginBottom: '8px' }}>{allTools.length} tool{allTools.length !== 1 ? 's' : ''} discovered</div>
            <div className="mcp-tools-list">
              {allTools.map((tool) => (
                <div key={tool.prefixed_name} className="mcp-tool-item">
                  <span className="mcp-tool-name">{tool.prefixed_name}</span>
                  <span className="mcp-tool-desc">{tool.description}</span>
                </div>
              ))}
            </div>
          </>
        )}
      </div>

      {/* Install Bridge Dependencies */}
      {bridgeStatus !== 'Running' && (
        <div className="settings-group">
          <h3>Bridge Dependencies <InfoTip text="Install the Node.js dependencies required by the Anthropic Bridge service." /></h3>
          <div className="status-indicator">
            <span className={`status-dot ${bridgeStatus === 'Running' ? 'healthy' : 'unhealthy'}`} />
            <span>Bridge: {bridgeStatus || 'Unknown'}</span>
          </div>
          <div className="action-buttons">
            <button className="primary" onClick={async () => {
              setInstallingBridge(true);
              try {
                await invoke('install_bridge');
                notifySuccess('Bridge', 'Dependencies installed successfully.');
              } catch (error) {
                notifyError('Bridge', `Failed to install bridge dependencies: ${error}`);
              } finally {
                setInstallingBridge(false);
              }
            }} disabled={installingBridge}>
              {installingBridge ? 'Installing...' : 'Install Bridge Dependencies'}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
