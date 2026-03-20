import React from 'react';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ToolsTab } from './ToolsTab';
import { invoke } from '@tauri-apps/api/core';
import { useSettings } from '../SettingsContext';

// -------------------------------------------------------------------
// Mocks
// -------------------------------------------------------------------
vi.mock('../SettingsContext');
vi.mock('../shared/InfoTip', () => ({ InfoTip: () => null }));

const mockConfirm = vi.hoisted(() => vi.fn(() => Promise.resolve(true)));
vi.mock('../../../shared/useConfirm', () => ({
  useConfirm: () => ({ confirm: mockConfirm, modal: null }),
}));

// -------------------------------------------------------------------
// Fixture helpers
// -------------------------------------------------------------------
const makeConfig = (overrides: Record<string, any> = {}) => ({
  search: {
    search_priority: ['brave', 'tavily', 'browser'],
    default_result_count: 5,
    brave_api_key: undefined as string | undefined,
    tavily_api_key: undefined as string | undefined,
  },
  computer_use: {
    enabled: false,
    display_width: 1280,
    display_height: 720,
    require_confirmation: true,
  },
  browser: {
    enabled: false,
    headless: true,
    default_timeout_ms: 30000,
    viewport_width: 1280,
    viewport_height: 720,
    require_confirmation: true,
    allowed_domains: [] as string[],
    use_guardrails: true,
    chrome_path: undefined as string | undefined,
  },
  mcp: {
    enabled: true,
    servers: [],
  },
  ...overrides,
});

function setupSettings(overrides: {
  config?: ReturnType<typeof makeConfig> | null;
  mcpServers?: any[];
  loadMcpServers?: ReturnType<typeof vi.fn>;
  checkAccessibility?: ReturnType<typeof vi.fn>;
  accessibilityPermissions?: boolean | null;
  platformInfo?: { os: string } | null;
  bridgeStatus?: string | null;
  setConfig?: ReturnType<typeof vi.fn>;
} = {}) {
  const config = overrides.config !== undefined ? overrides.config : makeConfig();
  const setConfig = overrides.setConfig ?? vi.fn();
  const loadMcpServers = overrides.loadMcpServers ?? vi.fn();
  const checkAccessibility = overrides.checkAccessibility ?? vi.fn();

  vi.mocked(useSettings).mockReturnValue({
    config,
    setConfig,
    mcpServers: overrides.mcpServers ?? [],
    loadMcpServers,
    checkAccessibility,
    accessibilityPermissions: overrides.accessibilityPermissions !== undefined
      ? overrides.accessibilityPermissions
      : undefined,
    platformInfo: overrides.platformInfo !== undefined ? overrides.platformInfo : { os: 'macos' },
    bridgeStatus: overrides.bridgeStatus !== undefined ? overrides.bridgeStatus : 'Running',
  } as ReturnType<typeof useSettings>);

  return { config, setConfig, loadMcpServers };
}

function makeMcpServer(overrides: Partial<{
  name: string;
  command: string;
  status: string | { Error: string };
  tool_count: number;
  tools: any[];
  enabled: boolean;
}> = {}) {
  return {
    name: overrides.name ?? 'my-server',
    command: overrides.command ?? 'npx',
    status: overrides.status ?? 'Disconnected',
    tool_count: overrides.tool_count ?? 0,
    tools: overrides.tools ?? [],
    enabled: overrides.enabled ?? true,
  };
}

// -------------------------------------------------------------------
// Global browser API stubs
// -------------------------------------------------------------------
beforeEach(() => {
  vi.clearAllMocks();
  window.alert = vi.fn();
  window.confirm = vi.fn().mockReturnValue(true);
  vi.mocked(invoke).mockResolvedValue(undefined);
});

// ===================================================================
// 1. Rendering
// ===================================================================
describe('Rendering', () => {
  it('returns null when config is null', () => {
    setupSettings({ config: null });
    const { container } = render(<ToolsTab />);
    expect(container.firstChild).toBeNull();
  });

  it('renders Web Search and MCP Servers headings', () => {
    setupSettings();
    render(<ToolsTab />);
    expect(screen.getByText('Web Search')).toBeInTheDocument();
    expect(screen.getByText('MCP Servers')).toBeInTheDocument();
  });
});

// ===================================================================
// 2. Web Search
// ===================================================================
describe('Web Search', () => {
  it('Brave API key input calls setConfig', async () => {
    const user = userEvent.setup();
    const { setConfig, config } = setupSettings();
    render(<ToolsTab />);

    const input = screen.getByPlaceholderText('BSA...');
    await user.type(input, 'my-brave-key');

    expect(setConfig).toHaveBeenCalled();
    const lastCall = setConfig.mock.calls[setConfig.mock.calls.length - 1][0];
    expect(lastCall.search).toMatchObject({ brave_api_key: expect.any(String) });
  });

  it('Tavily API key input calls setConfig', async () => {
    const user = userEvent.setup();
    const { setConfig } = setupSettings();
    render(<ToolsTab />);

    const input = screen.getByPlaceholderText('tvly-...');
    await user.type(input, 'my-tavily-key');

    expect(setConfig).toHaveBeenCalled();
    const lastCall = setConfig.mock.calls[setConfig.mock.calls.length - 1][0];
    expect(lastCall.search).toMatchObject({ tavily_api_key: expect.any(String) });
  });

  it('default_result_count number input calls setConfig', async () => {
    const user = userEvent.setup();
    const { setConfig } = setupSettings();
    render(<ToolsTab />);

    const inputs = screen.getAllByRole('spinbutton');
    // The default_result_count is the first number input
    const countInput = inputs[0];
    await user.clear(countInput);
    await user.type(countInput, '10');

    expect(setConfig).toHaveBeenCalled();
    const lastCall = setConfig.mock.calls[setConfig.mock.calls.length - 1][0];
    expect(lastCall.search).toBeDefined();
  });
});

// ===================================================================
// 3. Quick Add Presets
// ===================================================================
describe('Quick Add presets', () => {
  it('shows playwright preset when not already in mcpServers', () => {
    setupSettings({ mcpServers: [] });
    render(<ToolsTab />);
    expect(screen.getByText('playwright')).toBeInTheDocument();
  });

  it('clicking playwright preset calls add_mcp_server invoke and loadMcpServers', async () => {
    const user = userEvent.setup();
    const { loadMcpServers } = setupSettings({ mcpServers: [] });
    render(<ToolsTab />);

    const playwrightCard = screen.getByText('playwright').closest('.mcp-preset-card');
    expect(playwrightCard).not.toBeNull();
    await user.click(playwrightCard!);

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('add_mcp_server', {
        config: {
          name: 'playwright',
          enabled: true,
          command: 'npx',
          args: ['-y', '@playwright/mcp@latest'],
          env: {},
        },
      });
    });
    expect(loadMcpServers).toHaveBeenCalled();
  });

  it('playwright preset not shown when already in mcpServers', () => {
    setupSettings({
      mcpServers: [makeMcpServer({ name: 'playwright' })],
    });
    render(<ToolsTab />);
    // The preset card should not be shown (playwright already added)
    // "playwright" may appear as the server name — make sure it's not in the presets section
    const presetSection = screen.queryByText('Quick Add');
    // If both playwright and filesystem are in mcpServers, Quick Add section is hidden
    // Here only playwright is present, so filesystem preset should still show
    expect(presetSection).toBeInTheDocument();
    // filesystem preset should still be shown
    expect(screen.getByText('filesystem')).toBeInTheDocument();
  });
});

// ===================================================================
// 4. MCP empty state
// ===================================================================
describe('MCP empty state', () => {
  it('shows "No MCP servers configured" when mcpServers is empty and add form is hidden', () => {
    setupSettings({ mcpServers: [] });
    render(<ToolsTab />);
    expect(screen.getByText(/No MCP servers configured/i)).toBeInTheDocument();
  });
});

// ===================================================================
// 5. MCP server card
// ===================================================================
describe('MCP server card', () => {
  it('shows server name in card', () => {
    setupSettings({
      mcpServers: [makeMcpServer({ name: 'my-server' })],
    });
    render(<ToolsTab />);
    expect(screen.getByText('my-server')).toBeInTheDocument();
  });

  it('shows Connect button when server is disconnected', () => {
    setupSettings({
      mcpServers: [makeMcpServer({ status: 'Disconnected' })],
    });
    render(<ToolsTab />);
    expect(screen.getByRole('button', { name: 'Connect' })).toBeInTheDocument();
  });

  it('shows Disconnect button when server is connected', () => {
    setupSettings({
      mcpServers: [makeMcpServer({ status: 'Connected' })],
    });
    render(<ToolsTab />);
    expect(screen.getByRole('button', { name: 'Disconnect' })).toBeInTheDocument();
  });

  it('shows tool count for the server', () => {
    setupSettings({
      mcpServers: [makeMcpServer({ tool_count: 7 })],
    });
    render(<ToolsTab />);
    expect(screen.getByText('7 tools')).toBeInTheDocument();
  });
});

// ===================================================================
// 6. MCP connect/disconnect
// ===================================================================
describe('MCP connect/disconnect', () => {
  it('clicking Connect calls connect_mcp_server and loadMcpServers', async () => {
    const user = userEvent.setup();
    const { loadMcpServers } = setupSettings({
      mcpServers: [makeMcpServer({ name: 'my-server', status: 'Disconnected' })],
    });
    render(<ToolsTab />);

    await user.click(screen.getByRole('button', { name: 'Connect' }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('connect_mcp_server', { name: 'my-server' });
    });
    expect(loadMcpServers).toHaveBeenCalled();
  });

  it('clicking Disconnect calls disconnect_mcp_server and loadMcpServers', async () => {
    const user = userEvent.setup();
    const { loadMcpServers } = setupSettings({
      mcpServers: [makeMcpServer({ name: 'my-server', status: 'Connected' })],
    });
    render(<ToolsTab />);

    await user.click(screen.getByRole('button', { name: 'Disconnect' }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('disconnect_mcp_server', { name: 'my-server' });
    });
    expect(loadMcpServers).toHaveBeenCalled();
  });
});

// ===================================================================
// 7. MCP remove
// ===================================================================
describe('MCP remove', () => {
  it('clicking Remove shows confirm and calls remove_mcp_server', async () => {
    const user = userEvent.setup();
    mockConfirm.mockResolvedValue(true);
    const { loadMcpServers } = setupSettings({
      mcpServers: [makeMcpServer({ name: 'my-server' })],
    });
    render(<ToolsTab />);

    await user.click(screen.getByRole('button', { name: 'Remove' }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('remove_mcp_server', { name: 'my-server' });
    });
    expect(loadMcpServers).toHaveBeenCalled();
  });

  it('clicking Remove but cancelling confirm does not call invoke', async () => {
    const user = userEvent.setup();
    mockConfirm.mockResolvedValue(false);
    setupSettings({
      mcpServers: [makeMcpServer({ name: 'my-server' })],
    });
    render(<ToolsTab />);

    await user.click(screen.getByRole('button', { name: 'Remove' }));

    expect(vi.mocked(invoke)).not.toHaveBeenCalledWith('remove_mcp_server', expect.anything());
  });
});

// ===================================================================
// 8. MCP add form
// ===================================================================
describe('MCP add form', () => {
  it('clicking "+ Add MCP Server" shows the add form', async () => {
    const user = userEvent.setup();
    setupSettings({ mcpServers: [] });
    render(<ToolsTab />);

    await user.click(screen.getByRole('button', { name: '+ Add MCP Server' }));

    expect(screen.getByPlaceholderText(/Server name/i)).toBeInTheDocument();
    expect(screen.getByPlaceholderText(/Command \(e\.g\., npx\)/i)).toBeInTheDocument();
  });

  it('Add Server button is disabled when name and command are empty', async () => {
    const user = userEvent.setup();
    setupSettings({ mcpServers: [] });
    render(<ToolsTab />);

    await user.click(screen.getByRole('button', { name: '+ Add MCP Server' }));

    expect(screen.getByRole('button', { name: 'Add Server' })).toBeDisabled();
  });

  it('Add Server calls invoke with correct args, resets form, and calls loadMcpServers', async () => {
    const user = userEvent.setup();
    const { loadMcpServers } = setupSettings({ mcpServers: [] });
    render(<ToolsTab />);

    await user.click(screen.getByRole('button', { name: '+ Add MCP Server' }));

    const nameInput = screen.getByPlaceholderText(/Server name/i);
    const commandInput = screen.getByPlaceholderText(/Command \(e\.g\., npx\)/i);

    fireEvent.change(nameInput, { target: { value: 'my-new-server' } });
    fireEvent.change(commandInput, { target: { value: 'node' } });

    await user.click(screen.getByRole('button', { name: 'Add Server' }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('add_mcp_server', {
        config: {
          name: 'my-new-server',
          enabled: true,
          command: 'node',
          args: [],
          env: {},
        },
      });
    });
    expect(loadMcpServers).toHaveBeenCalled();
    // Form should be hidden
    expect(screen.queryByRole('button', { name: 'Add Server' })).not.toBeInTheDocument();
  });

  it('Cancel button hides the add form', async () => {
    const user = userEvent.setup();
    setupSettings({ mcpServers: [] });
    render(<ToolsTab />);

    await user.click(screen.getByRole('button', { name: '+ Add MCP Server' }));
    expect(screen.getByPlaceholderText(/Server name/i)).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'Cancel' }));

    expect(screen.queryByPlaceholderText(/Server name/i)).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: '+ Add MCP Server' })).toBeInTheDocument();
  });
});

// ===================================================================
// 9. MCP error status
// ===================================================================
describe('MCP error status', () => {
  it('shows error message when server status has Error', () => {
    setupSettings({
      mcpServers: [makeMcpServer({ status: { Error: 'Connection refused' } })],
    });
    render(<ToolsTab />);
    expect(screen.getByText('Connection refused')).toBeInTheDocument();
  });
});

// ===================================================================
// 10. Computer Use
// ===================================================================
describe('Computer Use', () => {
  it('enable checkbox calls setConfig with enabled=true', async () => {
    const user = userEvent.setup();
    const { setConfig, config } = setupSettings();
    render(<ToolsTab />);

    // Find the Computer Use enable checkbox (first checkbox in Computer Use section)
    const checkboxes = screen.getAllByRole('checkbox');
    // Computer Use enable is one of the checkboxes — find it by its label context
    const computerUseSection = screen.getByText('Computer Use').closest('.settings-group');
    expect(computerUseSection).not.toBeNull();
    const cuCheckbox = computerUseSection!.querySelector('input[type="checkbox"]') as HTMLInputElement;
    expect(cuCheckbox).not.toBeNull();

    fireEvent.click(cuCheckbox);

    expect(setConfig).toHaveBeenCalledWith(
      expect.objectContaining({
        computer_use: expect.objectContaining({ enabled: true }),
      })
    );
  });

  it('require_confirmation checkbox is shown when computer_use is enabled', () => {
    setupSettings({
      config: makeConfig({
        computer_use: { enabled: true, display_width: 1280, display_height: 720, require_confirmation: true },
      }),
    });
    render(<ToolsTab />);
    const computerUseSection = screen.getByText('Computer Use').closest('.settings-group');
    const checkboxes = computerUseSection!.querySelectorAll('input[type="checkbox"]');
    // Should have at least 2 checkboxes: enable and require_confirmation
    expect(checkboxes.length).toBeGreaterThanOrEqual(2);
  });

  it('Grant Access button calls request_accessibility_permissions on macOS with permissions=false', async () => {
    const user = userEvent.setup();
    setupSettings({
      config: makeConfig({
        computer_use: { enabled: true, display_width: 1280, display_height: 720, require_confirmation: true },
      }),
      platformInfo: { os: 'macos' },
      accessibilityPermissions: false,
    });
    render(<ToolsTab />);

    const grantButton = screen.getByRole('button', { name: 'Grant Access' });
    await user.click(grantButton);

    expect(vi.mocked(invoke)).toHaveBeenCalledWith('request_accessibility_permissions');
  });
});

// ===================================================================
// 11. Browser
// ===================================================================
describe('Browser', () => {
  it('enable checkbox calls setConfig with enabled=true', async () => {
    const { setConfig } = setupSettings();
    render(<ToolsTab />);

    const browserSection = screen.getByText('Browser').closest('.settings-group');
    expect(browserSection).not.toBeNull();
    const browserEnableCheckbox = browserSection!.querySelector('input[type="checkbox"]') as HTMLInputElement;
    expect(browserEnableCheckbox).not.toBeNull();

    fireEvent.click(browserEnableCheckbox);

    expect(setConfig).toHaveBeenCalledWith(
      expect.objectContaining({
        browser: expect.objectContaining({ enabled: true }),
      })
    );
  });

  it('require_confirmation checkbox is shown when browser is enabled', () => {
    setupSettings({
      config: makeConfig({
        browser: {
          enabled: true,
          headless: true,
          default_timeout_ms: 30000,
          viewport_width: 1280,
          viewport_height: 720,
          require_confirmation: true,
          allowed_domains: [],
          use_guardrails: true,
          chrome_path: undefined,
        },
      }),
    });
    render(<ToolsTab />);
    const browserSection = screen.getByText('Browser').closest('.settings-group');
    const checkboxes = browserSection!.querySelectorAll('input[type="checkbox"]');
    // Should have at least 2: enable + require_confirmation
    expect(checkboxes.length).toBeGreaterThanOrEqual(2);
  });

  it('headless checkbox calls setConfig', async () => {
    const { setConfig } = setupSettings({
      config: makeConfig({
        browser: {
          enabled: true,
          headless: true,
          default_timeout_ms: 30000,
          viewport_width: 1280,
          viewport_height: 720,
          require_confirmation: true,
          allowed_domains: [],
          use_guardrails: true,
          chrome_path: undefined,
        },
      }),
    });
    render(<ToolsTab />);

    // Find the headless checkbox by its label text
    const headlessLabel = screen.getByText(/Headless mode/i);
    const headlessCheckbox = headlessLabel.closest('label')!.querySelector('input[type="checkbox"]') as HTMLInputElement;
    expect(headlessCheckbox).not.toBeNull();

    fireEvent.click(headlessCheckbox);

    expect(setConfig).toHaveBeenCalledWith(
      expect.objectContaining({
        browser: expect.objectContaining({ headless: false }),
      })
    );
  });

  it('allowed_domains textarea calls setConfig', () => {
    const { setConfig } = setupSettings({
      config: makeConfig({
        browser: {
          enabled: true,
          headless: true,
          default_timeout_ms: 30000,
          viewport_width: 1280,
          viewport_height: 720,
          require_confirmation: true,
          allowed_domains: [],
          use_guardrails: true,
          chrome_path: undefined,
        },
      }),
    });
    render(<ToolsTab />);

    const textarea = screen.getByPlaceholderText(/example\.com/i);
    fireEvent.change(textarea, { target: { value: 'google.com' } });

    expect(setConfig).toHaveBeenCalled();
    const lastCall = setConfig.mock.calls[setConfig.mock.calls.length - 1][0];
    expect(lastCall.browser.allowed_domains).toContain('google.com');
  });
});

// ===================================================================
// 12. All Discovered Tools
// ===================================================================
describe('All Discovered Tools', () => {
  it('"Load All Tools" button calls list_mcp_tools', async () => {
    const user = userEvent.setup();
    vi.mocked(invoke).mockResolvedValue([]);
    setupSettings();
    render(<ToolsTab />);

    await user.click(screen.getByRole('button', { name: 'Load All Tools' }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('list_mcp_tools');
    });
  });

  it('shows tool names after loading', async () => {
    const user = userEvent.setup();
    vi.mocked(invoke).mockResolvedValue([
      { name: 'search', prefixed_name: 'playwright_search', description: 'Search tool', server_name: 'playwright' },
      { name: 'click', prefixed_name: 'playwright_click', description: 'Click tool', server_name: 'playwright' },
    ]);
    setupSettings();
    render(<ToolsTab />);

    await user.click(screen.getByRole('button', { name: 'Load All Tools' }));

    await waitFor(() => {
      expect(screen.getByText('playwright_search')).toBeInTheDocument();
      expect(screen.getByText('playwright_click')).toBeInTheDocument();
    });
  });
});

// ===================================================================
// 13. Bridge Dependencies
// ===================================================================
describe('Bridge Dependencies', () => {
  it('section is hidden when bridgeStatus is "Running"', () => {
    setupSettings({ bridgeStatus: 'Running' });
    render(<ToolsTab />);
    expect(screen.queryByText('Bridge Dependencies')).not.toBeInTheDocument();
  });

  it('section is shown when bridgeStatus is "Stopped"', () => {
    setupSettings({ bridgeStatus: 'Stopped' });
    render(<ToolsTab />);
    expect(screen.getByText('Bridge Dependencies')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Install Bridge Dependencies' })).toBeInTheDocument();
  });

  it('Install Bridge Dependencies button calls install_bridge', async () => {
    const user = userEvent.setup();
    setupSettings({ bridgeStatus: 'Stopped' });
    render(<ToolsTab />);

    await user.click(screen.getByRole('button', { name: 'Install Bridge Dependencies' }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('install_bridge');
    });
  });
});
