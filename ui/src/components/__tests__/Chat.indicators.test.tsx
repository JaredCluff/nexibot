/**
 * Chat.indicators.test.tsx
 *
 * Tests for chat:thinking, chat:progress, and chat:model-fallback events.
 *
 * These listeners are registered inside sendMessage(), so each test must trigger
 * a send first to register the handlers, then dispatch the event.
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
// MessageList renders message content so model-fallback test can find it in the DOM.
vi.mock('../MessageList', () => ({
  default: ({ messages }: { messages?: Array<{ id: string; content: string }> }) => (
    <div data-testid="message-list">
      {messages?.map((m) => <div key={m.id}>{m.content}</div>)}
    </div>
  ),
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
      case 'get_session_overrides':     return Promise.resolve({ model: null, thinking_budget: null, verbose: false, provider: null });
      case 'list_agents':               return Promise.resolve([]);
      case 'get_active_gui_agent':      return Promise.resolve('');
      case 'get_available_models':      return Promise.resolve([]);
      case 'get_voice_status':          return Promise.resolve(null);
      case 'get_context_usage':         return Promise.resolve(null);
      case 'send_message_with_events':  return new Promise(() => { /* stays pending until chat:complete */ });
      default:                          return Promise.resolve(undefined);
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

/** Type text in the textarea and click Send so sendMessage registers its listeners. */
async function triggerSend(text = 'hello') {
  const textarea = screen.getByRole('textbox');
  await userEvent.type(textarea, text);
  const sendBtn = screen.getByRole('button', { name: /send/i });
  await userEvent.click(sendBtn);
  // Yield to the microtask queue so all `await listen(...)` calls inside sendMessage complete.
  await act(async () => { await Promise.resolve(); });
}

// ─── Tests ────────────────────────────────────────────────────────────────────

describe('Chat indicator events', () => {
  it('shows Thinking indicator when chat:thinking is received', async () => {
    await renderChat();
    await triggerSend();

    await act(async () => {
      dispatchTauriEvent('chat:thinking', undefined);
    });

    await waitFor(() => {
      expect(screen.getByText(/Thinking/i)).toBeInTheDocument();
    });
  });

  it('clears Thinking indicator when chat:complete is received', async () => {
    await renderChat();
    await triggerSend();

    await act(async () => {
      dispatchTauriEvent('chat:thinking', undefined);
    });
    await waitFor(() => {
      expect(screen.getByText(/Thinking/i)).toBeInTheDocument();
    });

    await act(async () => {
      dispatchTauriEvent('chat:complete', { response: 'done', error: undefined });
    });

    await waitFor(() => {
      expect(screen.queryByText(/Thinking/i)).not.toBeInTheDocument();
    });
  });

  it('shows loop progress "Step N of M" when chat:progress is received', async () => {
    await renderChat();
    await triggerSend();

    await act(async () => {
      dispatchTauriEvent('chat:progress', { iteration: 2, total: 5, elapsed_secs: 1.2 });
    });

    await waitFor(() => {
      expect(screen.getByText(/Step 2 of 5/i)).toBeInTheDocument();
    });
  });

  it('clears loop progress when chat:complete is received', async () => {
    await renderChat();
    await triggerSend();

    await act(async () => {
      dispatchTauriEvent('chat:progress', { iteration: 3, total: 5, elapsed_secs: 2.0 });
    });
    await waitFor(() => {
      expect(screen.getByText(/Step 3 of 5/i)).toBeInTheDocument();
    });

    await act(async () => {
      dispatchTauriEvent('chat:complete', { response: 'done', error: undefined });
    });

    await waitFor(() => {
      expect(screen.queryByText(/Step/i)).not.toBeInTheDocument();
    });
  });

  it('appends model-fallback notice as inline message when chat:model-fallback is received', async () => {
    await renderChat();
    await triggerSend();

    await act(async () => {
      dispatchTauriEvent('chat:model-fallback', {
        from_model: 'claude-opus-4-6',
        to_model: 'claude-sonnet-4-6',
        reason: 'context limit exceeded',
      });
    });

    await waitFor(() => {
      expect(
        screen.getByText((text) =>
          text.includes('claude-opus-4-6') && text.includes('claude-sonnet-4-6')
        )
      ).toBeInTheDocument();
    });
  });
});
