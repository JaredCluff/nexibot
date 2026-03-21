import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, fireEvent } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { SystemTab } from './SystemTab';
import { invoke } from '@tauri-apps/api/core';
import { emit } from '@tauri-apps/api/event';
import { useSettings } from '../SettingsContext';

const mockConfirm = vi.hoisted(() => vi.fn(() => Promise.resolve(true)));
vi.mock('../../../shared/useConfirm', () => ({
  useConfirm: () => ({ confirm: mockConfirm, modal: null }),
}));
vi.mock('../SettingsContext');
vi.mock('../shared/InfoTip', () => ({ InfoTip: () => null }));
vi.mock('../shared/TagInput', () => ({
  TagInput: ({
    tags,
    onChange,
    placeholder,
  }: {
    tags: string[];
    onChange: (tags: string[]) => void;
    placeholder?: string;
  }) => (
    <div data-testid="tag-input">
      {tags.map((tag, i) => (
        <span key={i} data-testid="tag">
          {tag}
        </span>
      ))}
      <input
        placeholder={placeholder}
        onChange={(e) => {
          if (e.target.value.endsWith(',')) {
            onChange([...tags, e.target.value.slice(0, -1).trim()]);
          }
        }}
      />
    </div>
  ),
}));

// ─── Fixtures ─────────────────────────────────────────────────────────────────

const makeConfig = (overrides: Record<string, unknown> = {}) => ({
  startup: { k2k_agent_binary: 'kn-agent' },
  claude: { api_key: '', model: 'claude-opus-4-6', max_tokens: 8192, system_prompt: '' },
  audio: { enabled: true, sample_rate: 16000, channels: 1 },
  gateway: {
    enabled: false,
    port: 18792,
    bind_address: '127.0.0.1',
    auth_mode: 'Token',
    max_connections: 50,
  },
  sandbox: {
    enabled: false,
    image: 'debian:bookworm-slim',
    non_root_user: 'sandbox',
    memory_limit: '512m',
    cpu_limit: 1.0,
    network_mode: 'none',
    timeout_seconds: 60,
    blocked_paths: [],
  },
  ...overrides,
});

function setupSettings(
  overrides: {
    config?: Record<string, unknown>;
    settings?: Record<string, unknown>;
  } = {}
) {
  const config = makeConfig(overrides.config ?? {});
  const setConfig = vi.fn();

  vi.mocked(useSettings).mockReturnValue({
    config,
    setConfig,
    startupConfig: { nexibot_at_login: false, k2k_agent_at_login: false },
    loadStartupConfig: vi.fn(),
    bridgeStatus: 'Running',
    setBridgeStatus: vi.fn(),
    heartbeatConfig: { enabled: false, interval_seconds: 60 },
    setHeartbeatConfig: vi.fn(),
    heartbeatRunning: false,
    setHeartbeatRunning: vi.fn(),
    soulTemplates: [],
    currentSoul: '',
    setCurrentSoul: vi.fn(),
    setSaveMessage: vi.fn(),
    oauthProfiles: [],
    oauthStatus: null,
    loadOAuthData: vi.fn(),
    subscriptions: [],
    loadSubscriptions: vi.fn(),
    ...(overrides.settings ?? {}),
  } as ReturnType<typeof useSettings>);

  return { config, setConfig };
}

// ─── Tests ────────────────────────────────────────────────────────────────────

describe('SystemTab', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    window.alert = vi.fn();
    window.confirm = vi.fn().mockReturnValue(true);
    vi.mocked(invoke).mockResolvedValue(undefined);
  });

  // ── 1. Rendering ──────────────────────────────────────────────────────────

  describe('rendering', () => {
    it('returns null when config is null', () => {
      vi.mocked(useSettings).mockReturnValue({
        config: null,
        setConfig: vi.fn(),
        startupConfig: { nexibot_at_login: false, k2k_agent_at_login: false },
        loadStartupConfig: vi.fn(),
        bridgeStatus: 'Running',
        setBridgeStatus: vi.fn(),
        heartbeatConfig: null,
        setHeartbeatConfig: vi.fn(),
        heartbeatRunning: false,
        setHeartbeatRunning: vi.fn(),
        soulTemplates: [],
        currentSoul: '',
        setCurrentSoul: vi.fn(),
        setSaveMessage: vi.fn(),
        oauthProfiles: [],
        oauthStatus: null,
        loadOAuthData: vi.fn(),
        subscriptions: [],
        loadSubscriptions: vi.fn(),
      } as ReturnType<typeof useSettings>);
      const { container } = render(<SystemTab />);
      expect(container).toBeEmptyDOMElement();
    });

    it('renders all main section headings when config is present', () => {
      setupSettings();
      render(<SystemTab />);
      expect(screen.getByRole('heading', { name: /Startup/i })).toBeInTheDocument();
      expect(screen.getByRole('heading', { name: /Updates/i })).toBeInTheDocument();
      expect(screen.getByRole('heading', { name: /Authentication/i })).toBeInTheDocument();
      expect(screen.getByRole('heading', { name: /Subscriptions/i })).toBeInTheDocument();
      expect(screen.getByRole('heading', { name: /Audio Input/i })).toBeInTheDocument();
      expect(screen.getByRole('heading', { name: /Soul/i })).toBeInTheDocument();
      expect(screen.getByRole('heading', { name: /Bridge Service/i })).toBeInTheDocument();
      expect(screen.getByRole('heading', { name: /Heartbeat/i })).toBeInTheDocument();
      expect(screen.getByRole('heading', { name: /WebSocket Gateway/i })).toBeInTheDocument();
      expect(screen.getByRole('heading', { name: /Docker Sandbox/i })).toBeInTheDocument();
      expect(screen.getByRole('heading', { name: /Background Tasks/i })).toBeInTheDocument();
    });
  });

  // ── 2. Startup — autostart ────────────────────────────────────────────────

  describe('startup — autostart', () => {
    it('nexibot checkbox calls set_nexibot_autostart and loadStartupConfig on click', async () => {
      const user = userEvent.setup();
      const loadStartupConfig = vi.fn();
      vi.mocked(useSettings).mockReturnValue({
        ...vi.mocked(useSettings)(),
        config: makeConfig(),
        setConfig: vi.fn(),
        startupConfig: { nexibot_at_login: false, k2k_agent_at_login: false },
        loadStartupConfig,
        bridgeStatus: 'Running',
        setBridgeStatus: vi.fn(),
        heartbeatConfig: { enabled: false, interval_seconds: 60 },
        setHeartbeatConfig: vi.fn(),
        heartbeatRunning: false,
        setHeartbeatRunning: vi.fn(),
        soulTemplates: [],
        currentSoul: '',
        setCurrentSoul: vi.fn(),
        setSaveMessage: vi.fn(),
        oauthProfiles: [],
        oauthStatus: null,
        loadOAuthData: vi.fn(),
        subscriptions: [],
        loadSubscriptions: vi.fn(),
      } as ReturnType<typeof useSettings>);
      render(<SystemTab />);

      await user.click(screen.getByRole('checkbox', { name: /Launch NexiBot at login/i }));

      await waitFor(() =>
        expect(vi.mocked(invoke)).toHaveBeenCalledWith('set_nexibot_autostart', { enabled: true })
      );
      expect(loadStartupConfig).toHaveBeenCalled();
    });

    it('k2k checkbox calls set_k2k_agent_autostart and loadStartupConfig on click', async () => {
      const user = userEvent.setup();
      const loadStartupConfig = vi.fn();
      vi.mocked(useSettings).mockReturnValue({
        ...vi.mocked(useSettings)(),
        config: makeConfig(),
        setConfig: vi.fn(),
        startupConfig: { nexibot_at_login: false, k2k_agent_at_login: false },
        loadStartupConfig,
        bridgeStatus: 'Running',
        setBridgeStatus: vi.fn(),
        heartbeatConfig: { enabled: false, interval_seconds: 60 },
        setHeartbeatConfig: vi.fn(),
        heartbeatRunning: false,
        setHeartbeatRunning: vi.fn(),
        soulTemplates: [],
        currentSoul: '',
        setCurrentSoul: vi.fn(),
        setSaveMessage: vi.fn(),
        oauthProfiles: [],
        oauthStatus: null,
        loadOAuthData: vi.fn(),
        subscriptions: [],
        loadSubscriptions: vi.fn(),
      } as ReturnType<typeof useSettings>);
      render(<SystemTab />);

      await user.click(screen.getByRole('checkbox', { name: /Launch K2K Agent at login/i }));

      await waitFor(() =>
        expect(vi.mocked(invoke)).toHaveBeenCalledWith('set_k2k_agent_autostart', { enabled: true })
      );
      expect(loadStartupConfig).toHaveBeenCalled();
    });

    it('shows alert on nexibot autostart error', async () => {
      const user = userEvent.setup();
      setupSettings();
      vi.mocked(invoke).mockImplementation((cmd: string) => {
        if (cmd === 'set_nexibot_autostart') return Promise.reject(new Error('permission denied'));
        return Promise.resolve(undefined as any);
      });
      render(<SystemTab />);

      await user.click(screen.getByRole('checkbox', { name: /Launch NexiBot at login/i }));

      await waitFor(() =>
        expect(vi.mocked(emit)).toHaveBeenCalledWith(
          'notify:toast',
          expect.objectContaining({ message: expect.stringContaining('Failed to update NexiBot autostart') }),
        )
      );
    });

    it('shows alert on k2k autostart error', async () => {
      const user = userEvent.setup();
      setupSettings();
      vi.mocked(invoke).mockImplementation((cmd: string) => {
        if (cmd === 'set_k2k_agent_autostart') return Promise.reject(new Error('permission denied'));
        return Promise.resolve(undefined as any);
      });
      render(<SystemTab />);

      await user.click(screen.getByRole('checkbox', { name: /Launch K2K Agent at login/i }));

      await waitFor(() =>
        expect(vi.mocked(emit)).toHaveBeenCalledWith(
          'notify:toast',
          expect.objectContaining({ message: expect.stringContaining('Failed to update K2K Agent autostart') }),
        )
      );
    });
  });

  // ── 3. Updates — check ────────────────────────────────────────────────────

  describe('updates — check', () => {
    it('Check for Updates button calls check_for_updates', async () => {
      const user = userEvent.setup();
      setupSettings();
      render(<SystemTab />);

      await user.click(screen.getByRole('button', { name: /Check for Updates/i }));

      await waitFor(() =>
        expect(vi.mocked(invoke)).toHaveBeenCalledWith('check_for_updates')
      );
    });

    it('shows update version info when update is available', async () => {
      const user = userEvent.setup();
      setupSettings();
      vi.mocked(invoke).mockImplementation((cmd: string) => {
        if (cmd === 'check_for_updates') return Promise.resolve({
          version: '1.2.3',
          date: '2026-01-01',
          body: 'Bug fixes and improvements',
        } as any);
        return Promise.resolve(undefined as any);
      });
      render(<SystemTab />);

      await user.click(screen.getByRole('button', { name: /Check for Updates/i }));

      await waitFor(() =>
        expect(screen.getByText(/Version 1\.2\.3/i)).toBeInTheDocument()
      );
    });

    it('shows "You are up to date!" message when no update is available', async () => {
      const user = userEvent.setup();
      vi.mocked(invoke).mockResolvedValueOnce(null);
      const setSaveMessage = vi.fn();
      setupSettings({ settings: { setSaveMessage } });
      render(<SystemTab />);

      await user.click(screen.getByRole('button', { name: /Check for Updates/i }));

      await waitFor(() =>
        expect(setSaveMessage).toHaveBeenCalledWith('You are up to date!')
      );
    });
  });

  // ── 4. Updates — install ──────────────────────────────────────────────────

  describe('updates — install', () => {
    it('Install Update button is visible when update info is available', async () => {
      const user = userEvent.setup();
      setupSettings();
      vi.mocked(invoke).mockImplementation((cmd: string) => {
        if (cmd === 'check_for_updates') return Promise.resolve({
          version: '1.2.3',
          date: '2026-01-01',
          body: null,
        } as any);
        return Promise.resolve(undefined as any);
      });
      render(<SystemTab />);

      await user.click(screen.getByRole('button', { name: /Check for Updates/i }));

      await waitFor(() =>
        expect(screen.getByRole('button', { name: /Install Update/i })).toBeInTheDocument()
      );
    });

    it('Install Update button calls install_update and shows alert', async () => {
      const user = userEvent.setup();
      setupSettings();
      vi.mocked(invoke).mockImplementation((cmd: string) => {
        if (cmd === 'check_for_updates') return Promise.resolve({
          version: '1.2.3',
          date: null,
          body: null,
        } as any);
        return Promise.resolve(undefined as any);
      });
      render(<SystemTab />);

      await user.click(screen.getByRole('button', { name: /Check for Updates/i }));
      await waitFor(() =>
        expect(screen.getByRole('button', { name: /Install Update/i })).toBeInTheDocument()
      );

      await user.click(screen.getByRole('button', { name: /Install Update/i }));

      await waitFor(() =>
        expect(vi.mocked(invoke)).toHaveBeenCalledWith('install_update')
      );
      await waitFor(() =>
        expect(vi.mocked(emit)).toHaveBeenCalledWith(
          'notify:toast',
          expect.objectContaining({ message: expect.stringContaining('Update installed') }),
        )
      );
    });
  });

  // ── 5. Authentication — no profiles ───────────────────────────────────────

  describe('authentication — no profiles', () => {
    it('shows "No OAuth profiles configured" when oauthStatus is null and profiles is empty', () => {
      setupSettings({
        settings: { oauthProfiles: [], oauthStatus: null },
      });
      render(<SystemTab />);
      expect(
        screen.getByText(/No OAuth profiles configured/i)
      ).toBeInTheDocument();
    });
  });

  // ── 6. Authentication — OAuth status ──────────────────────────────────────

  describe('authentication — OAuth status indicator', () => {
    it('shows healthy status dot when oauthStatus is not expiring', () => {
      setupSettings({
        settings: {
          oauthStatus: {
            provider: 'anthropic',
            profile_name: 'default',
            is_expiring: false,
            expires_at: null,
          },
        },
      });
      const { container } = render(<SystemTab />);
      expect(container.querySelector('.status-dot.healthy')).toBeInTheDocument();
    });

    it('shows unhealthy status dot when oauthStatus is expiring', () => {
      setupSettings({
        settings: {
          oauthStatus: {
            provider: 'anthropic',
            profile_name: 'default',
            is_expiring: true,
            expires_at: null,
          },
        },
      });
      const { container } = render(<SystemTab />);
      expect(container.querySelector('.status-dot.unhealthy')).toBeInTheDocument();
    });
  });

  // ── 7. Authentication — profiles ──────────────────────────────────────────

  describe('authentication — profiles', () => {
    it('shows profile card with provider and profile name', () => {
      setupSettings({
        settings: {
          oauthProfiles: [
            { provider: 'anthropic', profile_name: 'my-profile', expires_at: null },
          ],
          oauthStatus: null,
        },
      });
      render(<SystemTab />);
      expect(screen.getByText('anthropic')).toBeInTheDocument();
      expect(screen.getByText('my-profile')).toBeInTheDocument();
    });

    it('Remove button calls remove_oauth_profile and loadOAuthData', async () => {
      const user = userEvent.setup();
      const loadOAuthData = vi.fn().mockResolvedValue(undefined);
      setupSettings({
        settings: {
          oauthProfiles: [
            { provider: 'anthropic', profile_name: 'my-profile', expires_at: null },
          ],
          oauthStatus: null,
          loadOAuthData,
        },
      });
      render(<SystemTab />);

      await user.click(screen.getByRole('button', { name: /Remove/i }));

      await waitFor(() =>
        expect(vi.mocked(invoke)).toHaveBeenCalledWith('remove_oauth_profile', {
          profileName: 'my-profile',
        })
      );
      await waitFor(() => expect(loadOAuthData).toHaveBeenCalled());
    });
  });

  // ── 8. Authentication — sign in ───────────────────────────────────────────

  describe('authentication — sign in', () => {
    it('Sign in with Anthropic calls start_oauth_flow and loadOAuthData', async () => {
      const user = userEvent.setup();
      const loadOAuthData = vi.fn().mockResolvedValue(undefined);
      setupSettings({ settings: { loadOAuthData } });
      render(<SystemTab />);

      await user.click(screen.getByRole('button', { name: /Sign in with Anthropic/i }));

      await waitFor(() =>
        expect(vi.mocked(invoke)).toHaveBeenCalledWith('start_oauth_flow')
      );
      await waitFor(() => expect(loadOAuthData).toHaveBeenCalled());
    });

    it('Use Claude CLI Auth calls start_claude_cli_auth and loadOAuthData', async () => {
      const user = userEvent.setup();
      const loadOAuthData = vi.fn().mockResolvedValue(undefined);
      setupSettings({ settings: { loadOAuthData } });
      render(<SystemTab />);

      await user.click(screen.getByRole('button', { name: /Use Claude CLI Auth/i }));

      await waitFor(() =>
        expect(vi.mocked(invoke)).toHaveBeenCalledWith('start_claude_cli_auth')
      );
      await waitFor(() => expect(loadOAuthData).toHaveBeenCalled());
    });
  });

  // ── 9. Authentication — API key ───────────────────────────────────────────

  describe('authentication — API key', () => {
    it('changing API key input calls setConfig with the new value', async () => {
      const { config, setConfig } = setupSettings();
      render(<SystemTab />);

      const apiKeyInput = screen.getByPlaceholderText(/sk-ant-.../i);
      fireEvent.change(apiKeyInput, { target: { value: 'sk-ant-test-key' } });

      expect(setConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          claude: expect.objectContaining({ api_key: 'sk-ant-test-key' }),
        })
      );
    });
  });

  // ── 10. Subscriptions ─────────────────────────────────────────────────────

  describe('subscriptions', () => {
    it('shows "No active subscriptions" when subscriptions is empty', () => {
      setupSettings({ settings: { subscriptions: [] } });
      render(<SystemTab />);
      expect(screen.getByText(/No active subscriptions/i)).toBeInTheDocument();
    });

    it('Refresh button calls refresh_subscriptions and loadSubscriptions', async () => {
      const user = userEvent.setup();
      const loadSubscriptions = vi.fn().mockResolvedValue(undefined);
      setupSettings({ settings: { loadSubscriptions } });
      render(<SystemTab />);

      // There may be multiple Refresh buttons; the subscriptions one is the first
      const refreshButtons = screen.getAllByRole('button', { name: /Refresh/i });
      await user.click(refreshButtons[0]);

      await waitFor(() =>
        expect(vi.mocked(invoke)).toHaveBeenCalledWith('refresh_subscriptions')
      );
      await waitFor(() => expect(loadSubscriptions).toHaveBeenCalled());
    });

    it('Manage button calls open_subscription_portal', async () => {
      const user = userEvent.setup();
      setupSettings();
      render(<SystemTab />);

      await user.click(screen.getByRole('button', { name: /^Manage$/i }));

      await waitFor(() =>
        expect(vi.mocked(invoke)).toHaveBeenCalledWith('open_subscription_portal')
      );
    });

    it('Check Status button calls check_subscription with provider and loadSubscriptions', async () => {
      const user = userEvent.setup();
      const loadSubscriptions = vi.fn().mockResolvedValue(undefined);
      setupSettings({
        settings: {
          subscriptions: [
            { provider: 'anthropic', tier: 'pro', status: 'active', expires_at: null },
          ],
          loadSubscriptions,
        },
      });
      render(<SystemTab />);

      await user.click(screen.getByRole('button', { name: /Check Status/i }));

      await waitFor(() =>
        expect(vi.mocked(invoke)).toHaveBeenCalledWith('check_subscription', {
          provider: 'anthropic',
        })
      );
      await waitFor(() => expect(loadSubscriptions).toHaveBeenCalled());
    });
  });

  // ── 11. Audio Input ───────────────────────────────────────────────────────

  describe('audio input', () => {
    it('shows sample_rate as readonly input', () => {
      setupSettings();
      render(<SystemTab />);
      const sampleRateInput = screen.getByDisplayValue('16000');
      expect(sampleRateInput).toHaveAttribute('readOnly');
    });

    it('shows channels as readonly input', () => {
      setupSettings();
      render(<SystemTab />);
      // Scope to the Audio Input section to avoid ambiguity with other inputs value=1
      const audioHeading = screen.getByRole('heading', { name: /Audio Input/i });
      const audioSection = audioHeading.closest('.settings-group') as HTMLElement;
      const channelsInputs = audioSection.querySelectorAll('input[type="number"]');
      // sample_rate=16000, channels=1
      const channelsInput = Array.from(channelsInputs).find(
        (el) => (el as HTMLInputElement).value === '1'
      ) as HTMLInputElement;
      expect(channelsInput).toBeDefined();
      expect(channelsInput).toHaveAttribute('readOnly');
    });
  });

  // ── 12. Soul / Personality ────────────────────────────────────────────────

  describe('soul / personality', () => {
    it('textarea shows currentSoul content', () => {
      setupSettings({
        settings: { currentSoul: 'You are a helpful assistant.' },
      });
      render(<SystemTab />);
      const textarea = screen.getByPlaceholderText(
        /Define NexiBot's personality/i
      ) as HTMLTextAreaElement;
      expect(textarea.value).toBe('You are a helpful assistant.');
    });

    it('changing textarea calls setCurrentSoul', async () => {
      const setCurrentSoul = vi.fn();
      setupSettings({ settings: { currentSoul: '', setCurrentSoul } });
      render(<SystemTab />);

      const textarea = screen.getByPlaceholderText(/Define NexiBot's personality/i);
      fireEvent.change(textarea, { target: { value: 'New soul content' } });

      expect(setCurrentSoul).toHaveBeenCalledWith('New soul content');
    });

    it('Save Soul button calls update_soul with currentSoul and then setSaveMessage', async () => {
      const user = userEvent.setup();
      const setSaveMessage = vi.fn();
      setupSettings({
        settings: { currentSoul: 'My soul content', setSaveMessage },
      });
      render(<SystemTab />);

      await user.click(screen.getByRole('button', { name: /Save Soul/i }));

      await waitFor(() =>
        expect(vi.mocked(invoke)).toHaveBeenCalledWith('update_soul', {
          newContent: 'My soul content',
        })
      );
      await waitFor(() => expect(setSaveMessage).toHaveBeenCalledWith('Soul saved'));
    });

    it('soul template select calls load_soul_template and setCurrentSoul', async () => {
      const user = userEvent.setup();
      const setCurrentSoul = vi.fn();
      setupSettings({
        settings: {
          soulTemplates: [{ name: 'friendly', description: 'A friendly assistant' }],
          currentSoul: '',
          setCurrentSoul,
        },
      });
      vi.mocked(invoke).mockImplementation((cmd: string) => {
        if (cmd === 'load_soul_template') return Promise.resolve({ content: 'Template soul content' } as any);
        return Promise.resolve(undefined as any);
      });
      render(<SystemTab />);

      await user.selectOptions(
        screen.getByRole('combobox', { name: /Soul Template/i }),
        'friendly'
      );

      await waitFor(() =>
        expect(vi.mocked(invoke)).toHaveBeenCalledWith('load_soul_template', {
          templateName: 'friendly',
        })
      );
      await waitFor(() => expect(setCurrentSoul).toHaveBeenCalledWith('Template soul content'));
    });
  });

  // ── 13. Bridge Service ────────────────────────────────────────────────────

  describe('bridge service', () => {
    it('shows Running status with healthy dot', () => {
      setupSettings({ settings: { bridgeStatus: 'Running' } });
      const { container } = render(<SystemTab />);
      expect(screen.getByText('Running')).toBeInTheDocument();
      expect(container.querySelector('.status-dot.healthy')).toBeInTheDocument();
    });

    it('Start button calls start_bridge and setBridgeStatus("Running")', async () => {
      const user = userEvent.setup();
      const setBridgeStatus = vi.fn();
      setupSettings({ settings: { bridgeStatus: 'Stopped', setBridgeStatus } });
      render(<SystemTab />);

      await user.click(screen.getByRole('button', { name: /^Start$/i }));

      await waitFor(() =>
        expect(vi.mocked(invoke)).toHaveBeenCalledWith('start_bridge')
      );
      expect(setBridgeStatus).toHaveBeenCalledWith('Running');
    });

    it('Stop button calls stop_bridge and setBridgeStatus("Stopped")', async () => {
      const user = userEvent.setup();
      const setBridgeStatus = vi.fn();
      setupSettings({ settings: { bridgeStatus: 'Running', setBridgeStatus } });
      render(<SystemTab />);

      await user.click(screen.getByRole('button', { name: /^Stop$/i }));

      await waitFor(() =>
        expect(vi.mocked(invoke)).toHaveBeenCalledWith('stop_bridge')
      );
      expect(setBridgeStatus).toHaveBeenCalledWith('Stopped');
    });

    it('Restart button calls restart_bridge and setBridgeStatus("Running")', async () => {
      const user = userEvent.setup();
      const setBridgeStatus = vi.fn();
      setupSettings({ settings: { bridgeStatus: 'Running', setBridgeStatus } });
      render(<SystemTab />);

      await user.click(screen.getByRole('button', { name: /^Restart$/i }));

      await waitFor(() =>
        expect(vi.mocked(invoke)).toHaveBeenCalledWith('restart_bridge')
      );
      expect(setBridgeStatus).toHaveBeenCalledWith('Running');
    });
  });

  // ── 14. Heartbeat ─────────────────────────────────────────────────────────

  describe('heartbeat', () => {
    it('shows inactive dot when heartbeatRunning is false', () => {
      setupSettings({ settings: { heartbeatRunning: false } });
      const { container } = render(<SystemTab />);
      // Stopped status dot in the Heartbeat section
      const heartbeatSection = screen.getByRole('heading', { name: /Heartbeat/i })
        .closest('.settings-group') as HTMLElement;
      expect(heartbeatSection.querySelector('.status-dot.inactive')).toBeInTheDocument();
    });

    it('shows healthy dot when heartbeatRunning is true', () => {
      setupSettings({ settings: { heartbeatRunning: true } });
      const { container } = render(<SystemTab />);
      const heartbeatSection = screen.getByRole('heading', { name: /Heartbeat/i })
        .closest('.settings-group') as HTMLElement;
      expect(heartbeatSection.querySelector('.status-dot.healthy')).toBeInTheDocument();
    });

    it('enable heartbeat checkbox calls setHeartbeatConfig with enabled: true', async () => {
      const user = userEvent.setup();
      const setHeartbeatConfig = vi.fn();
      setupSettings({
        settings: {
          heartbeatConfig: { enabled: false, interval_seconds: 60 },
          setHeartbeatConfig,
        },
      });
      render(<SystemTab />);

      await user.click(screen.getByRole('checkbox', { name: /Enable Heartbeat/i }));

      expect(setHeartbeatConfig).toHaveBeenCalledWith(
        expect.objectContaining({ enabled: true })
      );
    });

    it('interval input calls setHeartbeatConfig with new interval', () => {
      const setHeartbeatConfig = vi.fn();
      setupSettings({
        settings: {
          heartbeatConfig: { enabled: false, interval_seconds: 60 },
          setHeartbeatConfig,
        },
      });
      render(<SystemTab />);

      // Scope to Heartbeat section to avoid ambiguity with sandbox timeout_seconds=60
      const heartbeatHeading = screen.getByRole('heading', { name: /Heartbeat/i });
      const heartbeatSection = heartbeatHeading.closest('.settings-group') as HTMLElement;
      const intervalInput = heartbeatSection.querySelector('input[type="number"]') as HTMLInputElement;
      expect(intervalInput).toBeDefined();
      fireEvent.change(intervalInput, { target: { value: '120' } });

      expect(setHeartbeatConfig).toHaveBeenCalledWith(
        expect.objectContaining({ interval_seconds: 120 })
      );
    });

    it('Apply button calls update_heartbeat_config', async () => {
      const user = userEvent.setup();
      setupSettings({
        settings: {
          heartbeatConfig: { enabled: false, interval_seconds: 60 },
          heartbeatRunning: false,
        },
      });
      render(<SystemTab />);

      await user.click(screen.getByRole('button', { name: /Apply/i }));

      await waitFor(() =>
        expect(vi.mocked(invoke)).toHaveBeenCalledWith(
          'update_heartbeat_config',
          expect.objectContaining({ config: expect.any(Object) })
        )
      );
    });
  });

  // ── 15. WebSocket Gateway ─────────────────────────────────────────────────

  describe('websocket gateway', () => {
    it('enable gateway checkbox calls setConfig with enabled toggled', async () => {
      const user = userEvent.setup();
      const { setConfig, config } = setupSettings();
      render(<SystemTab />);

      await user.click(screen.getByRole('checkbox', { name: /Enable Gateway/i }));

      expect(setConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          gateway: expect.objectContaining({ enabled: true }),
        })
      );
    });

    it('shows warning when bind_address is 0.0.0.0', () => {
      setupSettings({
        config: {
          gateway: {
            enabled: false,
            port: 18792,
            bind_address: '0.0.0.0',
            auth_mode: 'Token',
            max_connections: 50,
          },
        },
      });
      render(<SystemTab />);
      expect(
        screen.getByText(/Binding to all interfaces exposes the gateway/i)
      ).toBeInTheDocument();
    });

    it('shows warning when auth_mode is Open', () => {
      setupSettings({
        config: {
          gateway: {
            enabled: false,
            port: 18792,
            bind_address: '127.0.0.1',
            auth_mode: 'Open',
            max_connections: 50,
          },
        },
      });
      render(<SystemTab />);
      expect(
        screen.getByText(/Open auth mode allows unauthenticated access/i)
      ).toBeInTheDocument();
    });

    it('port input calls setConfig with new port', () => {
      const { setConfig } = setupSettings();
      render(<SystemTab />);

      const portInput = screen.getByDisplayValue('18792');
      fireEvent.change(portInput, { target: { value: '19000' } });

      expect(setConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          gateway: expect.objectContaining({ port: 19000 }),
        })
      );
    });
  });

  // ── 16. Docker Sandbox ────────────────────────────────────────────────────

  describe('docker sandbox', () => {
    it('enable sandbox checkbox calls setConfig with enabled toggled', async () => {
      const user = userEvent.setup();
      const { setConfig } = setupSettings();
      render(<SystemTab />);

      await user.click(screen.getByRole('checkbox', { name: /Enable Sandbox/i }));

      expect(setConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          sandbox: expect.objectContaining({ enabled: true }),
        })
      );
    });

    it('Docker image input calls setConfig with new image value', () => {
      const { setConfig } = setupSettings();
      render(<SystemTab />);

      const imageInput = screen.getByDisplayValue('debian:bookworm-slim');
      fireEvent.change(imageInput, { target: { value: 'ubuntu:22.04' } });

      expect(setConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          sandbox: expect.objectContaining({ image: 'ubuntu:22.04' }),
        })
      );
    });

    it('network mode select calls setConfig with new network mode', async () => {
      const user = userEvent.setup();
      const { setConfig } = setupSettings();
      render(<SystemTab />);

      // The network mode select is the one in the sandbox section showing "none"
      const networkSelects = screen.getAllByRole('combobox');
      // Find the one with "none" as value
      const noneSelect = networkSelects.find(
        (el) => (el as HTMLSelectElement).value === 'none'
      ) as HTMLSelectElement;
      expect(noneSelect).toBeDefined();

      await user.selectOptions(noneSelect, 'bridge');

      expect(setConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          sandbox: expect.objectContaining({ network_mode: 'bridge' }),
        })
      );
    });
  });

  // ── 17. Background Tasks ──────────────────────────────────────────────────

  describe('background tasks', () => {
    it('Refresh button calls list_background_tasks', async () => {
      const user = userEvent.setup();
      setupSettings();
      vi.mocked(invoke).mockImplementation((cmd: string) => {
        if (cmd === 'list_background_tasks') return Promise.resolve([] as any);
        return Promise.resolve(undefined as any);
      });
      render(<SystemTab />);

      const refreshButtons = screen.getAllByRole('button', { name: /Refresh/i });
      // The last Refresh button belongs to Background Tasks section
      await user.click(refreshButtons[refreshButtons.length - 1]);

      await waitFor(() =>
        expect(vi.mocked(invoke)).toHaveBeenCalledWith('list_background_tasks')
      );
    });

    it('shows empty state text before loading tasks', () => {
      setupSettings();
      render(<SystemTab />);
      expect(
        screen.getByText(/No background tasks\. Click Refresh to check\./i)
      ).toBeInTheDocument();
    });

    it('shows task card with description after loading tasks', async () => {
      const user = userEvent.setup();
      setupSettings();
      vi.mocked(invoke).mockImplementation((cmd: string) => {
        if (cmd === 'list_background_tasks') return Promise.resolve([
          {
            id: 'task-1',
            description: 'Indexing documents',
            status: 'Running',
            created_at: '2026-01-01T00:00:00Z',
            updated_at: '2026-01-01T00:01:00Z',
            progress: null,
            result_summary: null,
          },
        ] as any);
        return Promise.resolve(undefined as any);
      });
      render(<SystemTab />);

      const refreshButtons = screen.getAllByRole('button', { name: /Refresh/i });
      await user.click(refreshButtons[refreshButtons.length - 1]);

      await waitFor(() =>
        expect(screen.getByText('Indexing documents')).toBeInTheDocument()
      );
    });
  });
});
