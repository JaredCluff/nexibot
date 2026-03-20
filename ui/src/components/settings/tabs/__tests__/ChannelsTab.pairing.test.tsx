/**
 * ChannelsTab.pairing.test.tsx
 *
 * E2E pairing flow:
 * - A pending pairing request is injected via mocked context.
 * - DM policy is set to Pairing on a channel so PairingSection is rendered.
 * - The user clicks Approve.
 * - approve_pairing_code is invoked with the correct code.
 */

import React from 'react';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ChannelsTab } from '../ChannelsTab';
import { invoke } from '@tauri-apps/api/core';
import { useSettings } from '../../SettingsContext';

// ─── Mocks ────────────────────────────────────────────────────────────────────

vi.mock('../../SettingsContext');
vi.mock('../../shared/InfoTip', () => ({ InfoTip: () => null }));
vi.mock('../../shared/ChannelCard', () => ({
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
      {children}
    </div>
  ),
}));
vi.mock('../../shared/ToolPolicySection', () => ({
  ToolPolicySection: () => <div data-testid="tool-policy-section" />,
}));
vi.mock('../../shared/TagInput', () => ({
  TagInput: () => null,
}));

// PairingSection: use the REAL component so approval button behaviour is tested.
// It reads pairingRequests from useSettings, which we control via the mock below.
vi.mock('../../shared/PairingSection', async (importOriginal) => {
  return importOriginal<typeof import('../../shared/PairingSection')>();
});
vi.mock('../../../shared/notify', () => ({
  notifyError: vi.fn(),
  notifyInfo: vi.fn(),
}));

// ─── Fixtures ─────────────────────────────────────────────────────────────────

const PAIRING_CODE = 'ABCD-1234-EFGH';

function makeConfig(signalDmPolicy: 'Pairing' | 'Allowlist' = 'Pairing') {
  return {
    telegram: { enabled: false, bot_token: '', allowed_chat_ids: [], admin_chat_ids: [], voice_enabled: false, dm_policy: 'Allowlist' as const, tool_policy: { denied_tools: [], allowed_tools: [], admin_bypass: true } },
    whatsapp: { enabled: false, phone_number_id: '', access_token: '', verify_token: '', app_secret: '', allowed_phone_numbers: [], admin_phone_numbers: [], dm_policy: 'Allowlist' as const, tool_policy: { denied_tools: [], allowed_tools: [], admin_bypass: true } },
    discord: { enabled: false, bot_token: '', allowed_guild_ids: [], allowed_channel_ids: [], admin_user_ids: [], dm_policy: 'Allowlist' as const, tool_policy: { denied_tools: [], allowed_tools: [], admin_bypass: true } },
    slack: { enabled: false, bot_token: '', app_token: '', signing_secret: '', allowed_channel_ids: [], admin_user_ids: [], dm_policy: 'Allowlist' as const, tool_policy: { denied_tools: [], allowed_tools: [], admin_bypass: true } },
    signal: { enabled: true, api_url: 'http://localhost:8080', phone_number: '+15551234567', allowed_numbers: [], admin_numbers: [], dm_policy: signalDmPolicy, tool_policy: { denied_tools: [], allowed_tools: [], admin_bypass: true } },
    teams: { enabled: false, app_id: '', app_password: '', admin_user_ids: [], dm_policy: 'Allowlist' as const, tool_policy: { denied_tools: [], allowed_tools: [], admin_bypass: true } },
    matrix: { enabled: false, homeserver_url: '', access_token: '', user_id: '', allowed_room_ids: [], admin_user_ids: [], dm_policy: 'Allowlist' as const, tool_policy: { denied_tools: [], allowed_tools: [], admin_bypass: true } },
    email: { enabled: false, imap_host: '', imap_port: 993, imap_username: '', imap_password: '', smtp_host: '', smtp_port: 587, smtp_username: '', smtp_password: '', from_address: '', allowed_senders: [], poll_interval_seconds: 30, folder: 'INBOX' },
    webhooks: { enabled: false, port: 18791 },
  };
}

function setupSettings(opts: { dmPolicy?: 'Pairing' | 'Allowlist'; withPairingRequest?: boolean } = {}) {
  const { dmPolicy = 'Pairing', withPairingRequest = true } = opts;
  const config = makeConfig(dmPolicy);
  const setConfig = vi.fn();
  const loadPairingData = vi.fn();

  vi.mocked(useSettings).mockReturnValue({
    config,
    setConfig,
    mcpServers: [],
    pairingRequests: withPairingRequest
      ? [{ id: 'pair-req-1', code: PAIRING_CODE, channel: 'signal', sender_id: '+14155551234', created_at: new Date().toISOString() }]
      : [],
    runtimeAllowlist: { telegram: [], whatsapp: [], channels: {} },
    loadPairingData,
  } as ReturnType<typeof useSettings>);

  return { config, setConfig, loadPairingData };
}

// ─── Tests ────────────────────────────────────────────────────────────────────

describe('ChannelsTab — pairing flow', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(invoke).mockResolvedValue(undefined);
  });

  it('shows pairing section when Signal DM policy is Pairing', () => {
    setupSettings({ dmPolicy: 'Pairing' });
    render(<ChannelsTab />);
    // PairingSection is shown inside the Signal card when dm_policy=Pairing.
    // The real PairingSection renders pending requests as rows with Approve/Deny buttons.
    expect(screen.getByText(PAIRING_CODE)).toBeInTheDocument();
  });

  it('calls approve_pairing_code with the correct code when Approve is clicked', async () => {
    const { loadPairingData } = setupSettings({ dmPolicy: 'Pairing' });
    render(<ChannelsTab />);

    const approveButton = screen.getByRole('button', { name: /approve/i });
    await userEvent.click(approveButton);

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('approve_pairing_code', { code: PAIRING_CODE });
    });
  });

  it('reloads pairing data after approval', async () => {
    const { loadPairingData } = setupSettings({ dmPolicy: 'Pairing' });
    render(<ChannelsTab />);

    const approveButton = screen.getByRole('button', { name: /approve/i });
    await userEvent.click(approveButton);

    await waitFor(() => {
      expect(loadPairingData).toHaveBeenCalled();
    });
  });

  it('calls deny_pairing_code with the correct code when Deny is clicked', async () => {
    setupSettings({ dmPolicy: 'Pairing' });
    render(<ChannelsTab />);

    const denyButton = screen.getByRole('button', { name: /deny/i });
    await userEvent.click(denyButton);

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('deny_pairing_code', { code: PAIRING_CODE });
    });
  });

  it('does not show PairingSection when DM policy is Allowlist', () => {
    setupSettings({ dmPolicy: 'Allowlist', withPairingRequest: true });
    render(<ChannelsTab />);
    // With Allowlist policy, PairingSection should not be rendered for Signal.
    expect(screen.queryByText(PAIRING_CODE)).not.toBeInTheDocument();
  });
});
