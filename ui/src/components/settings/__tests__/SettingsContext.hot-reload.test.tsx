/**
 * SettingsContext.hot-reload.test.tsx
 *
 * Verifies the config:changed hot-reload behaviour:
 * - Without unsaved changes → loadConfig() is called (config reloads silently).
 * - With unsaved changes    → setSaveMessage banner shown, loadConfig() NOT called.
 */

import React from 'react';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, waitFor, act } from '@testing-library/react';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';

// ─── Module under test ────────────────────────────────────────────────────────

// We import the actual context so we can render a consumer inside the provider.
import {
  SettingsProvider,
  useSettings,
} from '../SettingsContext';

// ─── Event bus ────────────────────────────────────────────────────────────────

type EventHandler = (event: { payload: any }) => void;
let eventHandlers: Map<string, EventHandler[]>;

function dispatchTauriEvent(event: string, payload: any = {}) {
  (eventHandlers.get(event) ?? []).forEach((h) => h({ payload }));
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/** A consumer component that exposes context values to the test via data-* attrs. */
function ContextConsumer() {
  const ctx = useSettings();
  return (
    <div>
      <span data-testid="save-message">{ctx.saveMessage}</span>
      <button
        data-testid="mark-dirty"
        onClick={() => {
          // Simulate a user edit by mutating config (any field change sets hasUnsavedChanges)
          ctx.setConfig({ ...ctx.config, claude: { ...ctx.config.claude, model: 'test-dirty-model' } } as any);
        }}
      >
        Mark dirty
      </button>
    </div>
  );
}

// ─── Setup ────────────────────────────────────────────────────────────────────

const MINIMAL_CONFIG = {
  config_version: 1,
  claude: { model: 'claude-sonnet-4-6' },
  telegram: { enabled: false, bot_token: '', allowed_chat_ids: [], admin_chat_ids: [], voice_enabled: false, dm_policy: 'Allowlist', tool_policy: { denied_tools: [], allowed_tools: [], admin_bypass: true } },
};

beforeEach(() => {
  eventHandlers = new Map();

  vi.mocked(listen).mockImplementation(async (event: string, handler: any) => {
    const existing = eventHandlers.get(event) ?? [];
    existing.push(handler);
    eventHandlers.set(event, existing);
    // Return an unlisten function that actually removes this specific handler
    // so that when a useEffect re-runs it can clean up the old listener.
    return vi.fn(() => {
      const handlers = eventHandlers.get(event) ?? [];
      const idx = handlers.indexOf(handler);
      if (idx !== -1) handlers.splice(idx, 1);
    });
  });

  vi.mocked(invoke).mockImplementation((cmd: string) => {
    switch (cmd) {
      case 'get_config':              return Promise.resolve(MINIMAL_CONFIG);
      case 'get_available_models':    return Promise.resolve([]);
      case 'get_defense_status':      return Promise.resolve({ defense_enabled: false, deberta_available: false, llama_guard_available: false, deberta_healthy: false });
      case 'get_scheduled_tasks':     return Promise.resolve([]);
      case 'get_scheduler_enabled':   return Promise.resolve(false);
      case 'get_startup_config':      return Promise.resolve({ nexibot_at_login: false, k2k_agent_at_login: false, k2k_agent_binary: '' });
      case 'get_pairing_requests':    return Promise.resolve([]);
      case 'list_agents':             return Promise.resolve([]);
      case 'get_oauth_profiles':      return Promise.resolve([]);
      case 'get_subscription_status': return Promise.resolve(null);
      case 'get_voice_status':        return Promise.resolve(null);
      case 'get_tool_permissions':    return Promise.resolve([]);
      case 'get_runtime_allowlist':   return Promise.resolve([]);
      case 'get_soul_templates':      return Promise.resolve([]);
      case 'get_skills':              return Promise.resolve([]);
      case 'get_mcp_servers':         return Promise.resolve([]);
      case 'check_supermemory_configured': return Promise.resolve(false);
      case 'get_clawhub_config':      return Promise.resolve(null);
      default:                        return Promise.resolve(undefined);
    }
  });
});

afterEach(() => {
  vi.clearAllMocks();
});

async function renderProvider() {
  render(
    <SettingsProvider>
      <ContextConsumer />
    </SettingsProvider>
  );
  // Let initial data-loading effects settle.
  await act(async () => {
    await new Promise(r => setTimeout(r, 50));
  });
}

// ─── Tests ────────────────────────────────────────────────────────────────────

describe('config:changed hot-reload', () => {
  it('reloads config silently when there are no unsaved changes', async () => {
    await renderProvider();

    const invokeCallsBefore = vi.mocked(invoke).mock.calls.filter(c => c[0] === 'get_config').length;

    await act(async () => {
      dispatchTauriEvent('config:changed');
    });

    await waitFor(() => {
      const calls = vi.mocked(invoke).mock.calls.filter(c => c[0] === 'get_config').length;
      expect(calls).toBeGreaterThan(invokeCallsBefore);
    });

    // No banner should appear.
    expect(screen.getByTestId('save-message').textContent).toBe('');
  });

  it('shows a banner and does NOT reload when there are unsaved changes', async () => {
    await renderProvider();

    // Simulate the user making a change; wrap in act so React processes the
    // state update and re-registers the config:changed listener with the
    // new hasUnsavedChanges=true value before we dispatch the event.
    await act(async () => {
      screen.getByTestId('mark-dirty').click();
    });

    const invokeCallsBefore = vi.mocked(invoke).mock.calls.filter(c => c[0] === 'get_config').length;

    await act(async () => {
      dispatchTauriEvent('config:changed');
    });

    await waitFor(() => {
      expect(screen.getByTestId('save-message').textContent).toMatch(/Config changed externally/i);
    });

    // get_config should NOT have been called again.
    const invokeCallsAfter = vi.mocked(invoke).mock.calls.filter(c => c[0] === 'get_config').length;
    expect(invokeCallsAfter).toBe(invokeCallsBefore);
  });
});
