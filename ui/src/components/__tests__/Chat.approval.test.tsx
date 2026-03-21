/**
 * Chat.approval.test.tsx
 *
 * E2E flow: chat:tool-approval-request → approval dialog visible →
 * respond_tool_approval invoked with the correct decision.
 */

import React from 'react';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, waitFor, act } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import Chat from '../Chat';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

// ─── Mocks ────────────────────────────────────────────────────────────────────

vi.mock('react-markdown', () => ({
  default: ({ children }: { children: React.ReactNode }) => <>{children}</>,
}));
vi.mock('../GuardrailsPanel', () => ({
  default: () => <div data-testid="guardrails-panel" />,
}));
vi.mock('../VoiceBar', () => ({
  default: () => <div data-testid="voice-bar" />,
}));
vi.mock('../MessageList', () => ({
  default: () => <div data-testid="message-list" />,
  extractDisplayText: (c: string) => c,
}));
vi.mock('../SlashCommandPalette', () => ({
  default: () => null,
}));
vi.mock('../../shared/notify', () => ({
  notifyError: vi.fn(),
  notifyInfo: vi.fn(),
}));

// ─── Event bus ────────────────────────────────────────────────────────────────

type EventHandler = (event: { payload: any }) => void;
let eventHandlers: Map<string, EventHandler[]>;

function dispatchTauriEvent(event: string, payload: any) {
  (eventHandlers.get(event) ?? []).forEach((h) => h({ payload }));
}

// ─── Setup ────────────────────────────────────────────────────────────────────

beforeEach(() => {
  eventHandlers = new Map();
  vi.mocked(listen).mockImplementation(async (event: string, handler: any) => {
    const list = eventHandlers.get(event) ?? [];
    list.push(handler);
    eventHandlers.set(event, list);
    return vi.fn();
  });

  vi.mocked(invoke).mockImplementation((cmd: string) => {
    switch (cmd) {
      case 'get_session_overrides':  return Promise.resolve({ model: null, thinking_budget: null, verbose: false, provider: null });
      case 'list_agents':            return Promise.resolve([]);
      case 'get_active_gui_agent':   return Promise.resolve('');
      case 'get_available_models':   return Promise.resolve([]);
      case 'get_voice_status':       return Promise.resolve(null);
      case 'get_context_usage':      return Promise.resolve(null);
      case 'respond_tool_approval':  return Promise.resolve(true);
      default:                       return Promise.resolve(undefined);
    }
  });
});

afterEach(() => {
  vi.clearAllMocks();
});

async function renderChat() {
  render(<Chat />);
  await act(async () => { await Promise.resolve(); });
}

// ─── Tests ────────────────────────────────────────────────────────────────────

describe('Tool-approval request flow', () => {
  it('shows the approval bar when chat:tool-approval-request is emitted', async () => {
    await renderChat();

    await act(async () => {
      dispatchTauriEvent('chat:tool-approval-request', {
        request_id: 'req-1',
        tool_name: 'nexibot_execute',
        reason: 'Run a shell command',
        timeout_secs: 30,
      });
    });

    await waitFor(() => {
      expect(screen.getByText(/Approve:.*Execute Command/i)).toBeInTheDocument();
      expect(screen.getByText(/Run a shell command/i)).toBeInTheDocument();
    });
  });

  it('invokes respond_tool_approval(approved=true) when Approve is clicked', async () => {
    await renderChat();

    await act(async () => {
      dispatchTauriEvent('chat:tool-approval-request', {
        request_id: 'req-2',
        tool_name: 'nexibot_fetch',
        reason: 'Fetch external URL',
      });
    });

    const approveButton = await screen.findByRole('button', { name: /approve/i });
    await userEvent.click(approveButton);

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('respond_tool_approval', {
        requestId: 'req-2',
        approved: true,
      });
    });
  });

  it('invokes respond_tool_approval(approved=false) when Deny is clicked', async () => {
    await renderChat();

    await act(async () => {
      dispatchTauriEvent('chat:tool-approval-request', {
        request_id: 'req-3',
        tool_name: 'nexibot_filesystem',
        reason: 'Write to /etc/hosts',
      });
    });

    const denyButton = await screen.findByRole('button', { name: /deny/i });
    await userEvent.click(denyButton);

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('respond_tool_approval', {
        requestId: 'req-3',
        approved: false,
      });
    });
  });

  it('removes the approval bar after responding', async () => {
    await renderChat();

    await act(async () => {
      dispatchTauriEvent('chat:tool-approval-request', {
        request_id: 'req-4',
        tool_name: 'nexibot_execute',
        reason: 'Execute code',
      });
    });

    expect(await screen.findByText(/Approve:/i)).toBeInTheDocument();

    const approveButton = screen.getByRole('button', { name: /approve/i });
    await userEvent.click(approveButton);

    await waitFor(() => {
      expect(screen.queryByText(/Approve:/i)).not.toBeInTheDocument();
    });
  });

  it('removes the approval bar when chat:tool-approval-expired is received', async () => {
    await renderChat();

    await act(async () => {
      dispatchTauriEvent('chat:tool-approval-request', {
        request_id: 'req-5',
        tool_name: 'nexibot_execute',
        reason: 'Will expire',
      });
    });

    expect(await screen.findByText(/Approve:/i)).toBeInTheDocument();

    await act(async () => {
      dispatchTauriEvent('chat:tool-approval-expired', { request_id: 'req-5' });
    });

    await waitFor(() => {
      expect(screen.queryByText(/Approve:/i)).not.toBeInTheDocument();
    });
  });
});
