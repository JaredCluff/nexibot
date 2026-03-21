import React from 'react';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ChannelsTab } from './ChannelsTab';
import { invoke } from '@tauri-apps/api/core';
import { useSettings } from '../SettingsContext';

// ─── Mocks ────────────────────────────────────────────────────────────────────

vi.mock('../SettingsContext');
vi.mock('../shared/InfoTip', () => ({ InfoTip: () => null }));
vi.mock('../shared/ChannelCard', () => ({
  ChannelCard: ({
    name,
    enabled,
    onToggle,
    children,
  }: {
    name: string;
    enabled: boolean;
    onToggle: (v: boolean) => void;
    children?: React.ReactNode;
  }) => (
    <div data-testid={`channel-${name.toLowerCase().replace(/\s+/g, '-')}`}>
      <input
        type="checkbox"
        aria-label={`Enable ${name}`}
        checked={enabled}
        onChange={(e) => onToggle(e.target.checked)}
      />
      <span>{name}</span>
      {children}
    </div>
  ),
}));
vi.mock('../shared/PairingSection', () => ({
  PairingSection: ({ channel }: { channel: string }) => (
    <div data-testid={`pairing-${channel}`} />
  ),
}));
vi.mock('../shared/ToolPolicySection', () => ({
  ToolPolicySection: () => <div data-testid="tool-policy-section" />,
}));
vi.mock('../shared/TagInput', () => ({
  TagInput: ({
    tags,
    onChange,
    placeholder,
  }: {
    tags: string[];
    onChange: (t: string[]) => void;
    placeholder?: string;
  }) => (
    <div>
      {(tags ?? []).map((t, i) => (
        <span key={i} className="tag">
          {t}
        </span>
      ))}
      <input
        placeholder={placeholder}
        onChange={(e) => onChange([...(tags ?? []), e.target.value])}
      />
    </div>
  ),
}));

// ─── Config fixture ───────────────────────────────────────────────────────────

const makeConfig = (overrides: Record<string, unknown> = {}) => ({
  telegram: {
    enabled: false,
    bot_token: '',
    allowed_chat_ids: [],
    admin_chat_ids: [],
    voice_enabled: false,
    dm_policy: 'Allowlist' as const,
    tool_policy: {
      denied_tools: ['nexibot_execute', 'nexibot_filesystem'],
      allowed_tools: [],
      admin_bypass: true,
    },
  },
  whatsapp: {
    enabled: false,
    phone_number_id: '',
    access_token: '',
    verify_token: '',
    app_secret: '',
    allowed_phone_numbers: [],
    admin_phone_numbers: [],
    dm_policy: 'Allowlist' as const,
    tool_policy: {
      denied_tools: ['nexibot_execute', 'nexibot_filesystem'],
      allowed_tools: [],
      admin_bypass: true,
    },
  },
  discord: {
    enabled: false,
    bot_token: '',
    allowed_guild_ids: [],
    allowed_channel_ids: [],
    admin_user_ids: [],
    dm_policy: 'Allowlist' as const,
    tool_policy: {
      denied_tools: ['nexibot_execute', 'nexibot_filesystem'],
      allowed_tools: [],
      admin_bypass: true,
    },
  },
  slack: {
    enabled: false,
    bot_token: '',
    app_token: '',
    allowed_user_ids: [],
    admin_user_ids: [],
    dm_policy: 'Allowlist' as const,
    tool_policy: {
      denied_tools: ['nexibot_execute', 'nexibot_filesystem'],
      allowed_tools: [],
      admin_bypass: true,
    },
  },
  signal: {
    enabled: false,
    api_url: '',
    phone_number: '',
    allowed_numbers: [],
    dm_policy: 'Allowlist' as const,
    tool_policy: {
      denied_tools: ['nexibot_execute', 'nexibot_filesystem'],
      allowed_tools: [],
      admin_bypass: true,
    },
  },
  teams: {
    enabled: false,
    app_id: '',
    app_password: '',
    dm_policy: 'Allowlist' as const,
    tool_policy: {
      denied_tools: ['nexibot_execute', 'nexibot_filesystem'],
      allowed_tools: [],
      admin_bypass: true,
    },
  },
  matrix: {
    enabled: false,
    homeserver_url: '',
    access_token: '',
    dm_policy: 'Allowlist' as const,
    tool_policy: {
      denied_tools: ['nexibot_execute', 'nexibot_filesystem'],
      allowed_tools: [],
      admin_bypass: true,
    },
  },
  email: {
    enabled: false,
    imap_host: '',
    imap_port: 993,
    smtp_host: '',
    smtp_port: 587,
    username: '',
    password: '',
    allowed_senders: [],
    dm_policy: 'Allowlist' as const,
    tool_policy: {
      denied_tools: ['nexibot_execute', 'nexibot_filesystem'],
      allowed_tools: [],
      admin_bypass: true,
    },
  },
  webhooks: { enabled: false, port: 18791, auth_token: undefined, tls: { enabled: false } },
  ...overrides,
});

function setupSettings(configOverrides: Record<string, unknown> = {}) {
  const config = makeConfig(configOverrides);
  const setConfig = vi.fn();

  vi.mocked(useSettings).mockReturnValue({
    config,
    setConfig,
    mcpServers: [],
    pairingRequests: [],
    runtimeAllowlist: {
      telegram: [],
      whatsapp: [],
      channels: {},
    },
    loadPairingData: vi.fn(),
  } as ReturnType<typeof useSettings>);

  return { config, setConfig };
}

// ─── Tests ────────────────────────────────────────────────────────────────────

describe('ChannelsTab', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    window.alert = vi.fn();
    vi.mocked(invoke).mockResolvedValue(undefined);
  });

  // ── Rendering ──────────────────────────────────────────────────────────────

  describe('rendering', () => {
    it('returns null when config is null', () => {
      vi.mocked(useSettings).mockReturnValue({
        config: null,
        setConfig: vi.fn(),
        mcpServers: [],
        pairingRequests: [],
        runtimeAllowlist: { telegram: [], whatsapp: [], channels: {} },
        loadPairingData: vi.fn(),
      } as ReturnType<typeof useSettings>);

      const { container } = render(<ChannelsTab />);
      expect(container).toBeEmptyDOMElement();
    });

    it('renders all 8 channel sections', () => {
      setupSettings();
      render(<ChannelsTab />);

      expect(screen.getByTestId('channel-telegram')).toBeInTheDocument();
      expect(screen.getByTestId('channel-whatsapp')).toBeInTheDocument();
      expect(screen.getByTestId('channel-discord')).toBeInTheDocument();
      expect(screen.getByTestId('channel-slack')).toBeInTheDocument();
      expect(screen.getByTestId('channel-signal')).toBeInTheDocument();
      expect(screen.getByTestId('channel-microsoft-teams')).toBeInTheDocument();
      expect(screen.getByTestId('channel-matrix')).toBeInTheDocument();
      expect(screen.getByTestId('channel-email')).toBeInTheDocument();
    });
  });

  // ── Telegram ───────────────────────────────────────────────────────────────

  describe('Telegram channel', () => {
    it('renders the Telegram channel card', () => {
      setupSettings();
      render(<ChannelsTab />);
      expect(screen.getByTestId('channel-telegram')).toBeInTheDocument();
      expect(screen.getByLabelText('Enable Telegram')).toBeInTheDocument();
    });

    it('toggle calls set_telegram_enabled and setConfig', async () => {
      const { setConfig } = setupSettings();
      render(<ChannelsTab />);

      fireEvent.click(screen.getByLabelText('Enable Telegram'));

      await waitFor(() =>
        expect(vi.mocked(invoke)).toHaveBeenCalledWith('set_telegram_enabled', { enabled: true })
      );
      expect(setConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          telegram: expect.objectContaining({ enabled: true }),
        })
      );
    });

    it('bot_token input change calls setConfig with updated bot_token', () => {
      const { setConfig } = setupSettings();
      render(<ChannelsTab />);

      const tokenInput = screen.getByPlaceholderText('123456:ABC-DEF1234ghIkl...');
      fireEvent.change(tokenInput, { target: { value: 'my-bot-token' } });

      expect(setConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          telegram: expect.objectContaining({ bot_token: 'my-bot-token' }),
        })
      );
    });

    it('voice_enabled toggle calls set_telegram_voice_enabled', async () => {
      const user = userEvent.setup();
      setupSettings();
      render(<ChannelsTab />);

      // Scope to the telegram card to find the voice checkbox (second checkbox, no aria-label)
      const telegramCard = screen.getByTestId('channel-telegram');
      const allCheckboxes = telegramCard.querySelectorAll('input[type="checkbox"]');
      // allCheckboxes[0] = "Enable Telegram" toggle, allCheckboxes[1] = voice enabled checkbox
      const voiceCheckbox = allCheckboxes[1] as HTMLElement;
      expect(voiceCheckbox).toBeTruthy();

      await user.click(voiceCheckbox);

      await waitFor(() =>
        expect(vi.mocked(invoke)).toHaveBeenCalledWith('set_telegram_voice_enabled', {
          enabled: true,
        })
      );
    });

    it('DM policy select calls set_telegram_dm_policy and setConfig', async () => {
      const { setConfig } = setupSettings();
      render(<ChannelsTab />);

      const telegramCard = screen.getByTestId('channel-telegram');
      const selects = telegramCard.querySelectorAll('select');
      expect(selects.length).toBeGreaterThan(0);
      const dmPolicySelect = selects[0];

      fireEvent.change(dmPolicySelect, { target: { value: 'Pairing' } });

      await waitFor(() =>
        expect(vi.mocked(invoke)).toHaveBeenCalledWith('set_telegram_dm_policy', {
          policy: 'Pairing',
        })
      );
      expect(setConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          telegram: expect.objectContaining({ dm_policy: 'Pairing' }),
        })
      );
    });

    it('Send Test Message button calls send_telegram_test_message', async () => {
      const user = userEvent.setup();
      vi.mocked(invoke).mockResolvedValueOnce('Message sent!');
      setupSettings();
      render(<ChannelsTab />);

      const btn = screen.getByRole('button', { name: /Send Test Message/i });
      await user.click(btn);

      await waitFor(() =>
        expect(vi.mocked(invoke)).toHaveBeenCalledWith('send_telegram_test_message')
      );
    });
  });

  // ── WhatsApp ───────────────────────────────────────────────────────────────

  describe('WhatsApp channel', () => {
    it('renders the WhatsApp channel card', () => {
      setupSettings();
      render(<ChannelsTab />);
      expect(screen.getByTestId('channel-whatsapp')).toBeInTheDocument();
      expect(screen.getByLabelText('Enable WhatsApp')).toBeInTheDocument();
    });

    it('toggle calls set_whatsapp_enabled and setConfig', async () => {
      const { setConfig } = setupSettings();
      render(<ChannelsTab />);

      fireEvent.click(screen.getByLabelText('Enable WhatsApp'));

      await waitFor(() =>
        expect(vi.mocked(invoke)).toHaveBeenCalledWith('set_whatsapp_enabled', { enabled: true })
      );
      expect(setConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          whatsapp: expect.objectContaining({ enabled: true }),
        })
      );
    });

    it('phone_number_id input change calls setConfig', () => {
      const { setConfig } = setupSettings();
      render(<ChannelsTab />);

      const input = screen.getByPlaceholderText('1234567890');
      fireEvent.change(input, { target: { value: '9876543210' } });

      expect(setConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          whatsapp: expect.objectContaining({ phone_number_id: '9876543210' }),
        })
      );
    });

    it('app secret blur calls set_whatsapp_app_secret', async () => {
      setupSettings();
      render(<ChannelsTab />);

      const whatsappCard = screen.getByTestId('channel-whatsapp');
      const input = within(whatsappCard).getByPlaceholderText('Meta app secret');
      fireEvent.blur(input);

      await waitFor(() =>
        expect(vi.mocked(invoke)).toHaveBeenCalledWith('set_whatsapp_app_secret', {
          appSecret: '',
        })
      );
    });

    it('DM policy select calls set_whatsapp_dm_policy and setConfig', async () => {
      const { setConfig } = setupSettings();
      render(<ChannelsTab />);

      const whatsappCard = screen.getByTestId('channel-whatsapp');
      const selects = whatsappCard.querySelectorAll('select');
      expect(selects.length).toBeGreaterThan(0);

      fireEvent.change(selects[0], { target: { value: 'Pairing' } });

      await waitFor(() =>
        expect(vi.mocked(invoke)).toHaveBeenCalledWith('set_whatsapp_dm_policy', {
          policy: 'Pairing',
        })
      );
      expect(setConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          whatsapp: expect.objectContaining({ dm_policy: 'Pairing' }),
        })
      );
    });
  });

  // ── Discord ────────────────────────────────────────────────────────────────

  describe('Discord channel', () => {
    it('renders the Discord channel card', () => {
      setupSettings();
      render(<ChannelsTab />);
      expect(screen.getByTestId('channel-discord')).toBeInTheDocument();
      expect(screen.getByLabelText('Enable Discord')).toBeInTheDocument();
    });

    it('toggle calls setConfig with discord.enabled updated', async () => {
      const { setConfig } = setupSettings();
      render(<ChannelsTab />);

      fireEvent.click(screen.getByLabelText('Enable Discord'));

      await waitFor(() => {
        expect(setConfig).toHaveBeenCalledWith(
          expect.objectContaining({
            discord: expect.objectContaining({ enabled: true }),
          })
        );
      });
    });

    it('bot_token input change calls setConfig for discord', () => {
      const { setConfig } = setupSettings();
      render(<ChannelsTab />);

      const discordCard = screen.getByTestId('channel-discord');
      const passwordInputs = discordCard.querySelectorAll('input[type="password"]');
      expect(passwordInputs.length).toBeGreaterThan(0);

      fireEvent.change(passwordInputs[0], { target: { value: 'discord-token-abc' } });

      expect(setConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          discord: expect.objectContaining({ bot_token: 'discord-token-abc' }),
        })
      );
    });

    it('DM policy select change invokes set_discord_dm_policy and calls setConfig', async () => {
      const { setConfig } = setupSettings();
      render(<ChannelsTab />);

      const discordCard = screen.getByTestId('channel-discord');
      const selects = discordCard.querySelectorAll('select');
      expect(selects.length).toBeGreaterThan(0);

      fireEvent.change(selects[0], { target: { value: 'Pairing' } });

      await waitFor(() => {
        expect(vi.mocked(invoke)).toHaveBeenCalledWith('set_discord_dm_policy', { policy: 'Pairing' });
        expect(setConfig).toHaveBeenCalledWith(
          expect.objectContaining({
            discord: expect.objectContaining({ dm_policy: 'Pairing' }),
          })
        );
      });
    });
  });

  // ── Slack ──────────────────────────────────────────────────────────────────

  describe('Slack channel', () => {
    it('renders the Slack channel card', () => {
      setupSettings();
      render(<ChannelsTab />);
      expect(screen.getByTestId('channel-slack')).toBeInTheDocument();
      expect(screen.getByLabelText('Enable Slack')).toBeInTheDocument();
    });

    it('toggle calls setConfig with slack.enabled updated', async () => {
      const { setConfig } = setupSettings();
      render(<ChannelsTab />);

      fireEvent.click(screen.getByLabelText('Enable Slack'));

      await waitFor(() => {
        expect(setConfig).toHaveBeenCalledWith(
          expect.objectContaining({
            slack: expect.objectContaining({ enabled: true }),
          })
        );
      });
    });

    it('bot_token input change calls setConfig for slack', () => {
      const { setConfig } = setupSettings();
      render(<ChannelsTab />);

      const slackCard = screen.getByTestId('channel-slack');
      const passwordInputs = slackCard.querySelectorAll('input[type="password"]');
      expect(passwordInputs.length).toBeGreaterThan(0);

      fireEvent.change(passwordInputs[0], { target: { value: 'xoxb-slack-token' } });

      expect(setConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          slack: expect.objectContaining({ bot_token: 'xoxb-slack-token' }),
        })
      );
    });
  });

  // ── Signal ─────────────────────────────────────────────────────────────────

  describe('Signal channel', () => {
    it('renders the Signal channel card', () => {
      setupSettings();
      render(<ChannelsTab />);
      expect(screen.getByTestId('channel-signal')).toBeInTheDocument();
      expect(screen.getByLabelText('Enable Signal')).toBeInTheDocument();
    });

    it('toggle calls setConfig with signal.enabled updated', () => {
      const { setConfig } = setupSettings();
      render(<ChannelsTab />);

      fireEvent.click(screen.getByLabelText('Enable Signal'));

      expect(setConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          signal: expect.objectContaining({ enabled: true }),
        })
      );
    });

    it('phone_number input change calls setConfig for signal', () => {
      const { setConfig } = setupSettings();
      render(<ChannelsTab />);

      // Scope to Signal card to avoid ambiguity with similar placeholders in other channels
      const signalCard = screen.getByTestId('channel-signal');
      // The phone_number input uses placeholder "+15551234567" as a text input (not textarea)
      const phoneInput = signalCard.querySelector('input[placeholder="+15551234567"]') as HTMLInputElement;
      expect(phoneInput).toBeTruthy();

      fireEvent.change(phoneInput, { target: { value: '+19998887777' } });

      expect(setConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          signal: expect.objectContaining({ phone_number: '+19998887777' }),
        })
      );
    });
  });

  // ── Microsoft Teams ────────────────────────────────────────────────────────

  describe('Microsoft Teams channel', () => {
    it('renders the Microsoft Teams channel card', () => {
      setupSettings();
      render(<ChannelsTab />);
      expect(screen.getByTestId('channel-microsoft-teams')).toBeInTheDocument();
      expect(screen.getByLabelText('Enable Microsoft Teams')).toBeInTheDocument();
    });

    it('toggle calls setConfig with teams.enabled updated', () => {
      const { setConfig } = setupSettings();
      render(<ChannelsTab />);

      fireEvent.click(screen.getByLabelText('Enable Microsoft Teams'));

      expect(setConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          teams: expect.objectContaining({ enabled: true }),
        })
      );
    });

    it('app_id input change calls setConfig for teams', () => {
      const { setConfig } = setupSettings();
      render(<ChannelsTab />);

      const appIdInput = screen.getByPlaceholderText('xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx');
      fireEvent.change(appIdInput, { target: { value: 'my-app-id-123' } });

      expect(setConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          teams: expect.objectContaining({ app_id: 'my-app-id-123' }),
        })
      );
    });
  });

  // ── Matrix ─────────────────────────────────────────────────────────────────

  describe('Matrix channel', () => {
    it('renders the Matrix channel card', () => {
      setupSettings();
      render(<ChannelsTab />);
      expect(screen.getByTestId('channel-matrix')).toBeInTheDocument();
      expect(screen.getByLabelText('Enable Matrix')).toBeInTheDocument();
    });

    it('toggle calls setConfig with matrix.enabled updated', () => {
      const { setConfig } = setupSettings();
      render(<ChannelsTab />);

      fireEvent.click(screen.getByLabelText('Enable Matrix'));

      expect(setConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          matrix: expect.objectContaining({ enabled: true }),
        })
      );
    });

    it('homeserver_url input change calls setConfig for matrix', () => {
      const { setConfig } = setupSettings();
      render(<ChannelsTab />);

      const homeserverInput = screen.getByPlaceholderText('https://matrix.org');
      fireEvent.change(homeserverInput, { target: { value: 'https://my.homeserver.org' } });

      expect(setConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          matrix: expect.objectContaining({ homeserver_url: 'https://my.homeserver.org' }),
        })
      );
    });
  });

  // ── Email ──────────────────────────────────────────────────────────────────

  describe('Email channel', () => {
    it('renders the Email channel card', () => {
      setupSettings();
      render(<ChannelsTab />);
      expect(screen.getByTestId('channel-email')).toBeInTheDocument();
      expect(screen.getByLabelText('Enable Email')).toBeInTheDocument();
    });

    it('toggle calls setConfig with email.enabled updated', () => {
      const { setConfig } = setupSettings();
      render(<ChannelsTab />);

      fireEvent.click(screen.getByLabelText('Enable Email'));

      expect(setConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          email: expect.objectContaining({ enabled: true }),
        })
      );
    });

    it('smtp_host input change calls setConfig for email', () => {
      const { setConfig } = setupSettings();
      render(<ChannelsTab />);

      const smtpInput = screen.getByPlaceholderText('smtp.gmail.com');
      fireEvent.change(smtpInput, { target: { value: 'smtp.myserver.com' } });

      expect(setConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          email: expect.objectContaining({ smtp_host: 'smtp.myserver.com' }),
        })
      );
    });

    it('imap_host input change calls setConfig for email', () => {
      const { setConfig } = setupSettings();
      render(<ChannelsTab />);

      const imapInput = screen.getByPlaceholderText('imap.gmail.com');
      fireEvent.change(imapInput, { target: { value: 'imap.myserver.com' } });

      expect(setConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          email: expect.objectContaining({ imap_host: 'imap.myserver.com' }),
        })
      );
    });
  });

  // ── PairingSection visibility ──────────────────────────────────────────────

  describe('PairingSection visibility', () => {
    it('shows PairingSection for telegram when dm_policy is Pairing', () => {
      setupSettings({
        telegram: {
          ...makeConfig().telegram,
          dm_policy: 'Pairing' as const,
        },
      });
      render(<ChannelsTab />);
      expect(screen.getByTestId('pairing-telegram')).toBeInTheDocument();
    });

    it('does not show PairingSection for telegram when dm_policy is Allowlist', () => {
      setupSettings({
        telegram: {
          ...makeConfig().telegram,
          dm_policy: 'Allowlist' as const,
        },
      });
      render(<ChannelsTab />);
      expect(screen.queryByTestId('pairing-telegram')).not.toBeInTheDocument();
    });
  });

  // ── ToolPolicySection ──────────────────────────────────────────────────────

  describe('ToolPolicySection', () => {
    it('renders ToolPolicySection for primary and extended security channels', () => {
      setupSettings();
      render(<ChannelsTab />);
      // Primary cards: Telegram, WhatsApp, Discord, Slack, Signal, Teams, Matrix, Email, Gmail = 9
      // Extended security list: BlueBubbles, Google Chat, Mattermost, Messenger,
      // Instagram, LINE, Twilio, Mastodon, Rocket.Chat, WebChat = 10
      const toolPolicySections = screen.getAllByTestId('tool-policy-section');
      expect(toolPolicySections).toHaveLength(19);
    });
  });

  describe('Additional channel security', () => {
    it('renders extended channel security controls', () => {
      setupSettings();
      render(<ChannelsTab />);

      expect(screen.getByText('Additional Channel Security')).toBeInTheDocument();
      expect(screen.getByLabelText('Enable BlueBubbles')).toBeInTheDocument();
      expect(screen.getByLabelText('Enable WebChat')).toBeInTheDocument();
    });

    it('updates BlueBubbles security fields through setConfig', () => {
      const { setConfig } = setupSettings({
        bluebubbles: {
          enabled: false,
          allowed_handles: [],
          admin_handles: [],
          dm_policy: 'Allowlist' as const,
          tool_policy: {
            denied_tools: ['nexibot_execute', 'nexibot_filesystem'],
            allowed_tools: [],
            admin_bypass: true,
          },
        },
      });
      render(<ChannelsTab />);

      fireEvent.click(screen.getByLabelText('Enable BlueBubbles'));
      expect(setConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          bluebubbles: expect.objectContaining({ enabled: true }),
        })
      );

      const bluebubblesCard = screen
        .getByText('BlueBubbles')
        .closest('.mcp-server-card') as HTMLElement;
      const textareas = bluebubblesCard.querySelectorAll('textarea');
      fireEvent.change(textareas[0], { target: { value: '+15551234567\nuser@example.com' } });
      expect(setConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          bluebubbles: expect.objectContaining({
            allowed_handles: ['+15551234567', 'user@example.com'],
          }),
        })
      );

      const selects = bluebubblesCard.querySelectorAll('select');
      fireEvent.change(selects[0], { target: { value: 'Pairing' } });
      expect(setConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          bluebubbles: expect.objectContaining({ dm_policy: 'Pairing' }),
        })
      );
    });

    it('shows pairing section for extended channels in Pairing mode', () => {
      setupSettings({
        bluebubbles: {
          enabled: true,
          allowed_handles: [],
          admin_handles: [],
          dm_policy: 'Pairing' as const,
          tool_policy: {
            denied_tools: ['nexibot_execute', 'nexibot_filesystem'],
            allowed_tools: [],
            admin_bypass: true,
          },
        },
      });
      render(<ChannelsTab />);

      expect(screen.getByTestId('pairing-bluebubbles')).toBeInTheDocument();
    });

    it('updates Google Chat credential fields through setConfig', () => {
      const { setConfig } = setupSettings({
        google_chat: {
          enabled: false,
          incoming_webhook_url: '',
          verification_token: '',
          allowed_spaces: [],
          admin_user_ids: [],
          dm_policy: 'Allowlist' as const,
          tool_policy: {
            denied_tools: ['nexibot_execute', 'nexibot_filesystem'],
            allowed_tools: [],
            admin_bypass: true,
          },
        },
      });
      render(<ChannelsTab />);

      const googleChatCard = screen
        .getByText('Google Chat')
        .closest('.mcp-server-card') as HTMLElement;
      const webhookInput = googleChatCard.querySelector(
        'input[placeholder^="https://chat.googleapis.com"]'
      ) as HTMLInputElement;

      expect(webhookInput).toBeTruthy();
      fireEvent.change(webhookInput, {
        target: { value: 'https://chat.googleapis.com/v1/spaces/abc/messages?key=123' },
      });

      expect(setConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          google_chat: expect.objectContaining({
            incoming_webhook_url:
              'https://chat.googleapis.com/v1/spaces/abc/messages?key=123',
          }),
        })
      );
    });
  });
});
