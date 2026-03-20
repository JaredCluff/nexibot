/**
 * Chat.task-events.test.tsx
 *
 * E2E flow: task:started / task:progress / task:complete events →
 * correct Chat UI state transitions (task pills appear, update, disappear).
 */

import React from 'react';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, waitFor, act } from '@testing-library/react';
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
  vi.useFakeTimers({ shouldAdvanceTime: true });
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
      default:                       return Promise.resolve(undefined);
    }
  });
});

afterEach(() => {
  vi.useRealTimers();
  vi.clearAllMocks();
});

async function renderChat() {
  render(<Chat />);
  await act(async () => { await Promise.resolve(); });
}

// ─── Tests ────────────────────────────────────────────────────────────────────

describe('Background task event transitions', () => {
  it('shows a task pill when task:started is emitted', async () => {
    await renderChat();

    await act(async () => {
      dispatchTauriEvent('task:started', {
        id: 'task-1',
        description: 'Scanning files',
        status: 'running',
      });
    });

    await waitFor(() => {
      expect(screen.getByText(/Scanning files/i)).toBeInTheDocument();
    });
  });

  it('updates the task pill text when task:progress is emitted', async () => {
    await renderChat();

    await act(async () => {
      dispatchTauriEvent('task:started', {
        id: 'task-2',
        description: 'Indexing',
        status: 'running',
      });
    });

    await act(async () => {
      dispatchTauriEvent('task:progress', {
        task_id: 'task-2',
        progress: '42 files indexed',
      });
    });

    await waitFor(() => {
      expect(screen.getByText(/42 files indexed/i)).toBeInTheDocument();
    });
  });

  it('marks the task as completed when task:complete is emitted', async () => {
    await renderChat();

    await act(async () => {
      dispatchTauriEvent('task:started', {
        id: 'task-3',
        description: 'Uploading',
        status: 'running',
      });
    });

    await waitFor(() => {
      expect(screen.getByText(/Uploading/i)).toBeInTheDocument();
    });

    await act(async () => {
      dispatchTauriEvent('task:complete', {
        task_id: 'task-3',
        summary: 'Upload done',
      });
    });

    // After 5s the completed task pill is removed.
    await act(async () => { vi.advanceTimersByTime(5000); });

    await waitFor(() => {
      expect(screen.queryByText(/Uploading/i)).not.toBeInTheDocument();
    });
  });

  it('marks the task as failed when task:failed is emitted and removes it after 5s', async () => {
    await renderChat();

    await act(async () => {
      dispatchTauriEvent('task:started', {
        id: 'task-4',
        description: 'Broken task',
        status: 'running',
      });
    });

    await waitFor(() => {
      expect(screen.getByText(/Broken task/i)).toBeInTheDocument();
    });

    await act(async () => {
      dispatchTauriEvent('task:failed', { task_id: 'task-4' });
    });

    await act(async () => { vi.advanceTimersByTime(5000); });

    await waitFor(() => {
      expect(screen.queryByText(/Broken task/i)).not.toBeInTheDocument();
    });
  });
});
