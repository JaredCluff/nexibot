import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, waitFor, act, fireEvent } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import Chat from './Chat';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

// ─── Module mocks ─────────────────────────────────────────────────────────────

// Note: @tauri-apps/api/core and @tauri-apps/api/event are already mocked in
// src/test/setup.ts — we only need to re-mock modules specific to this component.
vi.mock('react-markdown', () => ({
  default: ({ children }: { children: React.ReactNode }) => <>{children}</>,
}));
vi.mock('./GuardrailsPanel', () => ({
  default: ({ onClose, onApplied }: { onClose: () => void; onApplied: () => void }) => (
    <div data-testid="guardrails-panel">
      <button onClick={onClose}>Close Guardrails</button>
      <button onClick={onApplied}>Apply Guardrails</button>
    </div>
  ),
}));
vi.mock('./CodeBlock', () => ({
  default: ({ code }: { code: string }) => <pre>{code}</pre>,
}));

// ─── Event bus ────────────────────────────────────────────────────────────────

type EventHandler = (event: { payload: any }) => void;
let eventHandlers: Map<string, EventHandler[]>;

function dispatchTauriEvent(event: string, payload: any) {
  (eventHandlers.get(event) ?? []).forEach((h) => h({ payload }));
}

// ─── Fixtures ─────────────────────────────────────────────────────────────────

function makeVoiceStatus(overrides: Partial<{
  state: string;
  stt_backend: string;
  tts_backend: string;
  is_sleeping: boolean;
  voice_response_enabled: boolean;
  wakeword_enabled: boolean;
}> = {}) {
  return {
    state: 'Idle',
    stt_backend: 'macos',
    tts_backend: 'macos',
    is_sleeping: false,
    voice_response_enabled: true,
    wakeword_enabled: false,
    ...overrides,
  };
}

function defaultOverrides() {
  return { model: null, thinking_budget: null, verbose: false, provider: null };
}

// ─── Default invoke behaviour ─────────────────────────────────────────────────

function setupDefaultInvokes(overrides: Record<string, () => any> = {}) {
  vi.mocked(invoke).mockImplementation((cmd: string) => {
    if (overrides[cmd]) return Promise.resolve(overrides[cmd]());
    switch (cmd) {
      case 'get_session_overrides':  return Promise.resolve(defaultOverrides());
      case 'list_agents':            return Promise.resolve([]);
      case 'get_active_gui_agent':   return Promise.resolve('');
      case 'get_available_models':   return Promise.resolve([]);
      case 'get_voice_status':       return Promise.resolve(null);
      case 'get_context_usage':      return Promise.resolve(null);
      default:                       return Promise.resolve(undefined);
    }
  });
}

// ─── Setup / teardown ─────────────────────────────────────────────────────────

beforeEach(() => {
  eventHandlers = new Map();
  vi.mocked(listen).mockImplementation(async (event: string, handler: any) => {
    const list = eventHandlers.get(event) ?? [];
    list.push(handler);
    eventHandlers.set(event, list);
    return vi.fn();
  });

  vi.mocked(invoke).mockClear();
  setupDefaultInvokes();
});

afterEach(() => {
  // Safety net: ensure real timers are restored even if a test leaked fake timers
  vi.useRealTimers();
  vi.clearAllMocks();
});

// Helper: render Chat and let initial async effects settle
async function renderChat(props: Record<string, any> = {}) {
  render(<Chat {...props} />);
  await act(async () => { await Promise.resolve(); });
}

// Helper: render Chat with a running voice service (waits for the 500ms poll to fire)
async function renderChatWithVoice(voiceStatusOverrides: Parameters<typeof makeVoiceStatus>[0] = {}) {
  const voiceStatus = makeVoiceStatus(voiceStatusOverrides);
  setupDefaultInvokes({ get_voice_status: () => voiceStatus });
  render(<Chat />);
  await act(async () => { await Promise.resolve(); });
  // Wait for the 500ms poll to fire and update the voice bar
  await screen.findByRole('button', { name: 'Stop voice service' }, { timeout: 1500 });
}

// ─── Initial render ───────────────────────────────────────────────────────────

describe('Initial render', () => {
  it('shows the welcome screen when there are no messages', async () => {
    await renderChat();
    expect(screen.getByText('NexiBot')).toBeInTheDocument();
    expect(screen.getByText(/AI assistant with tools, memory, and voice/i)).toBeInTheDocument();
  });

  it('shows the welcome hints (Voice, PTT, Commands, Models)', async () => {
    await renderChat();
    expect(screen.getByText('Voice')).toBeInTheDocument();
    expect(screen.getByText('Push-to-talk')).toBeInTheDocument();
    expect(screen.getByText('Commands')).toBeInTheDocument();
    expect(screen.getByText('Models')).toBeInTheDocument();
  });

  it('renders the message textarea with correct placeholder', async () => {
    await renderChat();
    expect(screen.getByPlaceholderText(/Message NexiBot/i)).toBeInTheDocument();
  });

  it('Send button is disabled when textarea is empty', async () => {
    await renderChat();
    expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled();
  });

  it('shows "Voice off" in the voice bar', async () => {
    await renderChat();
    expect(screen.getByText('Voice off')).toBeInTheDocument();
  });

  it('calls get_session_overrides, list_agents, and get_available_models on mount', async () => {
    await renderChat();
    expect(vi.mocked(invoke)).toHaveBeenCalledWith('get_session_overrides');
    expect(vi.mocked(invoke)).toHaveBeenCalledWith('list_agents');
    expect(vi.mocked(invoke)).toHaveBeenCalledWith('get_available_models');
  });
});

// ─── Message sending ──────────────────────────────────────────────────────────

describe('Message sending', () => {
  it('Send button becomes enabled when text is typed', async () => {
    const user = userEvent.setup();
    await renderChat();

    const textarea = screen.getByPlaceholderText(/Message NexiBot/i);
    await user.type(textarea, 'Hello');

    expect(screen.getByRole('button', { name: 'Send' })).not.toBeDisabled();
  });

  it('pressing Enter calls send_message_with_events', async () => {
    const user = userEvent.setup();
    await renderChat();

    await user.type(screen.getByPlaceholderText(/Message NexiBot/i), 'Hello');
    await user.keyboard('{Enter}');

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('send_message_with_events', expect.objectContaining({
        request: expect.objectContaining({ message: 'Hello' }),
      }));
    });
  });

  it('Shift+Enter does not send (inserts newline)', async () => {
    const user = userEvent.setup();
    await renderChat();

    await user.type(screen.getByPlaceholderText(/Message NexiBot/i), 'Hello');
    await user.keyboard('{Shift>}{Enter}{/Shift}');

    // Should NOT have been called
    expect(vi.mocked(invoke)).not.toHaveBeenCalledWith('send_message_with_events', expect.anything());
  });

  it('user message appears immediately after send', async () => {
    const user = userEvent.setup();
    await renderChat();

    await user.type(screen.getByPlaceholderText(/Message NexiBot/i), 'Test message');
    await user.keyboard('{Enter}');

    expect(screen.getByText('Test message')).toBeInTheDocument();
  });

  it('input is cleared after sending', async () => {
    const user = userEvent.setup();
    await renderChat();

    const textarea = screen.getByPlaceholderText(/Message NexiBot/i) as HTMLTextAreaElement;
    await user.type(textarea, 'Hello there');
    await user.keyboard('{Enter}');

    expect(textarea.value).toBe('');
  });

  it('shows typing indicator (loading state) while waiting for response', async () => {
    const user = userEvent.setup();
    await renderChat();

    await user.type(screen.getByPlaceholderText(/Message NexiBot/i), 'Hi');
    await user.keyboard('{Enter}');

    await waitFor(() => {
      expect(screen.getByRole('button', { name: /■ Stop/i })).toBeInTheDocument();
    });
  });

  it('assistant response message appears after chat:complete event', async () => {
    const user = userEvent.setup();
    await renderChat();

    await user.type(screen.getByPlaceholderText(/Message NexiBot/i), 'Hello');
    await user.keyboard('{Enter}');

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('send_message_with_events', expect.anything());
    });

    act(() => {
      dispatchTauriEvent('chat:complete', { response: 'Hello back!', error: undefined });
    });

    await waitFor(() => {
      expect(screen.getByText('Hello back!')).toBeInTheDocument();
    });
  });

  it('welcome screen disappears after first message', async () => {
    const user = userEvent.setup();
    await renderChat();

    await user.type(screen.getByPlaceholderText(/Message NexiBot/i), 'Hi');
    await user.keyboard('{Enter}');

    expect(screen.queryByText('NexiBot')).not.toBeInTheDocument();
  });
});

describe('Tool approval flow', () => {
  it('renders approval bar and submits approval decision', async () => {
    const user = userEvent.setup();
    await renderChat();

    await user.type(screen.getByPlaceholderText(/Message NexiBot/i), 'run browser action');
    await user.keyboard('{Enter}');

    act(() => {
      dispatchTauriEvent('chat:tool-approval-request', {
        request_id: 'req-1',
        tool_name: 'browser_click',
        reason: 'External action requires confirmation',
      });
    });

    expect(screen.getByText('Approval required for browser_click')).toBeInTheDocument();
    expect(screen.getByText('External action requires confirmation')).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'Approve' }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('respond_tool_approval', {
        requestId: 'req-1',
        approved: true,
      });
    });
  });

  it('queues multiple approval requests and handles them in order', async () => {
    const user = userEvent.setup();
    await renderChat();

    act(() => {
      dispatchTauriEvent('chat:tool-approval-request', {
        request_id: 'req-1',
        tool_name: 'browser_click',
        reason: 'First approval needed',
      });
      dispatchTauriEvent('chat:tool-approval-request', {
        request_id: 'req-2',
        tool_name: 'filesystem_write',
        reason: 'Second approval needed',
      });
    });

    expect(screen.getByText('Approval required for browser_click')).toBeInTheDocument();
    expect(screen.getByText('First approval needed')).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'Deny' }));
    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('respond_tool_approval', {
        requestId: 'req-1',
        approved: false,
      });
    });

    await waitFor(() => {
      expect(screen.getByText('Approval required for filesystem_write')).toBeInTheDocument();
    });
    expect(screen.getByText('Second approval needed')).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'Approve' }));
    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('respond_tool_approval', {
        requestId: 'req-2',
        approved: true,
      });
    });
  });
});

// ─── Streaming ────────────────────────────────────────────────────────────────

describe('Streaming response', () => {
  it('accumulates text from chat:text-chunk events', async () => {
    const user = userEvent.setup();
    await renderChat();

    await user.type(screen.getByPlaceholderText(/Message NexiBot/i), 'Hi');
    await user.keyboard('{Enter}');

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('send_message_with_events', expect.anything());
    });

    act(() => {
      dispatchTauriEvent('chat:text-chunk', { text: 'Hello ' });
      dispatchTauriEvent('chat:text-chunk', { text: 'there!' });
    });

    await waitFor(() => {
      expect(screen.getByText('Hello there!')).toBeInTheDocument();
    });
  });

  it('shows tool indicator when chat:tool-start fires', async () => {
    const user = userEvent.setup();
    await renderChat();

    await user.type(screen.getByPlaceholderText(/Message NexiBot/i), 'Do something');
    await user.keyboard('{Enter}');

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('send_message_with_events', expect.anything());
    });

    act(() => {
      dispatchTauriEvent('chat:tool-start', { name: 'web_search', id: 'tool-1' });
    });

    await waitFor(() => {
      expect(screen.getByText('web_search')).toBeInTheDocument();
    });
  });
});

// ─── Stop / cancel ────────────────────────────────────────────────────────────

describe('Stop stream', () => {
  it('Stop button calls cancel_message', async () => {
    const user = userEvent.setup();
    await renderChat();

    await user.type(screen.getByPlaceholderText(/Message NexiBot/i), 'Hi');
    await user.keyboard('{Enter}');

    const stopBtn = await screen.findByRole('button', { name: /■ Stop/i });
    await user.click(stopBtn);

    expect(vi.mocked(invoke)).toHaveBeenCalledWith('cancel_message');
  });
});

// ─── Error handling ───────────────────────────────────────────────────────────

describe('Error handling', () => {
  it('error response renders with error content', async () => {
    const user = userEvent.setup();
    await renderChat();

    await user.type(screen.getByPlaceholderText(/Message NexiBot/i), 'Hi');
    await user.keyboard('{Enter}');

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('send_message_with_events', expect.anything());
    });

    act(() => {
      dispatchTauriEvent('chat:complete', { response: '', error: 'rate_limit_exceeded' });
    });

    await waitFor(() => {
      expect(screen.getByText(/Error:.*rate_limit_exceeded/i)).toBeInTheDocument();
    });
  });

  it('calls onAuthRequired when auth error is received', async () => {
    const onAuthRequired = vi.fn();
    const user = userEvent.setup();
    await renderChat({ onAuthRequired });

    await user.type(screen.getByPlaceholderText(/Message NexiBot/i), 'Hi');
    await user.keyboard('{Enter}');

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('send_message_with_events', expect.anything());
    });

    act(() => {
      dispatchTauriEvent('chat:complete', { response: '', error: 'No Claude authentication configured' });
    });

    await waitFor(() => {
      expect(onAuthRequired).toHaveBeenCalled();
    });
  });
});

// ─── Message actions ──────────────────────────────────────────────────────────

describe('Message actions', () => {
  async function sendAndReceive(text = 'Ping') {
    const user = userEvent.setup();
    await renderChat();

    await user.type(screen.getByPlaceholderText(/Message NexiBot/i), text);
    await user.keyboard('{Enter}');

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('send_message_with_events', expect.anything());
    });

    act(() => {
      dispatchTauriEvent('chat:complete', { response: 'Pong!', error: undefined });
    });

    await waitFor(() => screen.getByText('Pong!'));
    return user;
  }

  it('copy button (⎘) is rendered on each message', async () => {
    await sendAndReceive();
    const copyBtns = screen.getAllByTitle('Copy');
    expect(copyBtns.length).toBeGreaterThanOrEqual(2); // user + assistant
  });

  it('clicking copy writes text to clipboard', async () => {
    const user = await sendAndReceive('Hello test');
    // userEvent.setup() replaces navigator.clipboard with its own stub; spy on the stub's writeText
    const writeTextSpy = vi.spyOn(navigator.clipboard, 'writeText').mockResolvedValue(undefined);
    const copyBtns = screen.getAllByTitle('Copy');
    await user.click(copyBtns[0]);
    expect(writeTextSpy).toHaveBeenCalledWith('Hello test');
  });

  it('retry button (↺) appears on last user message only', async () => {
    await sendAndReceive();
    const retryBtns = screen.getAllByTitle('Edit and retry');
    expect(retryBtns).toHaveLength(1);
  });

  it('clicking retry restores user message text to input', async () => {
    const user = await sendAndReceive('Retry me');
    const retryBtn = screen.getByTitle('Edit and retry');
    await user.click(retryBtn);

    const textarea = screen.getByPlaceholderText(/Message NexiBot/i) as HTMLTextAreaElement;
    expect(textarea.value).toBe('Retry me');
  });
});

// ─── Slash command palette ────────────────────────────────────────────────────

describe('Slash command palette', () => {
  it('shows palette when "/" is typed', async () => {
    const user = userEvent.setup();
    await renderChat();

    await user.type(screen.getByPlaceholderText(/Message NexiBot/i), '/');

    expect(screen.getByText('/model')).toBeInTheDocument();
    expect(screen.getByText('/help')).toBeInTheDocument();
    expect(screen.getByText('/new')).toBeInTheDocument();
  });

  it('filters palette to matching commands', async () => {
    const user = userEvent.setup();
    await renderChat();

    await user.type(screen.getByPlaceholderText(/Message NexiBot/i), '/mod');

    expect(screen.getByText('/model')).toBeInTheDocument();
    expect(screen.queryByText('/help')).not.toBeInTheDocument();
  });

  it('palette hides when input has a space (command selected)', async () => {
    const user = userEvent.setup();
    await renderChat();

    await user.type(screen.getByPlaceholderText(/Message NexiBot/i), '/model ');

    // The palette should be hidden; check that no cmd-palette span contains '/model'
    expect(screen.queryByText('/model', { selector: 'span' })).not.toBeInTheDocument();
  });

  it('Tab selects the highlighted command', async () => {
    const user = userEvent.setup();
    await renderChat();

    const textarea = screen.getByPlaceholderText(/Message NexiBot/i) as HTMLTextAreaElement;
    await user.type(textarea, '/new');
    await user.keyboard('{Tab}');

    expect(textarea.value).toBe('/new ');
  });

  it('Escape clears input and closes palette', async () => {
    const user = userEvent.setup();
    await renderChat();

    const textarea = screen.getByPlaceholderText(/Message NexiBot/i) as HTMLTextAreaElement;
    await user.type(textarea, '/h');
    await user.keyboard('{Escape}');

    expect(textarea.value).toBe('');
    expect(screen.queryByText('/help')).not.toBeInTheDocument();
  });
});

// ─── Slash commands ───────────────────────────────────────────────────────────

describe('Slash commands', () => {
  it('/help shows available commands message', async () => {
    const user = userEvent.setup();
    await renderChat();

    await user.type(screen.getByPlaceholderText(/Message NexiBot/i), '/help');
    await user.keyboard('{Enter}');

    await waitFor(() => {
      expect(screen.getByText(/Available commands/i)).toBeInTheDocument();
    });
  });

  it('/new clears messages and calls create_named_session', async () => {
    const user = userEvent.setup();
    await renderChat();

    // First send a message to populate the chat
    await user.type(screen.getByPlaceholderText(/Message NexiBot/i), 'Hi');
    await user.keyboard('{Enter}');
    await waitFor(() => expect(screen.getByText('Hi')).toBeInTheDocument());

    // Resolve the pending send so isLoading becomes false before running /new
    act(() => {
      dispatchTauriEvent('chat:complete', { response: 'Hello!', error: undefined });
    });
    await waitFor(() => expect(screen.getByText('Hello!')).toBeInTheDocument());

    // Now run /new
    await user.type(screen.getByPlaceholderText(/Message NexiBot/i), '/new');
    await user.keyboard('{Enter}');

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('create_named_session', expect.anything());
    });
    expect(screen.queryByText('Hi')).not.toBeInTheDocument();
    expect(screen.getByText(/Started a new conversation/i)).toBeInTheDocument();
  });

  it('/guardrails shows the GuardrailsPanel', async () => {
    const user = userEvent.setup();
    await renderChat();

    await user.type(screen.getByPlaceholderText(/Message NexiBot/i), '/guardrails');
    await user.keyboard('{Enter}');

    expect(screen.getByTestId('guardrails-panel')).toBeInTheDocument();
  });

  it('/compact calls compact_conversation', async () => {
    const user = userEvent.setup();
    setupDefaultInvokes({
      compact_conversation: () => ({
        success: true,
        messages_before: 10,
        messages_after: 3,
        tokens_before: 5000,
        tokens_after: 1200,
      }),
    });
    await renderChat();

    await user.type(screen.getByPlaceholderText(/Message NexiBot/i), '/compact');
    await user.keyboard('{Enter}');

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('compact_conversation');
    });
    await waitFor(() => {
      expect(screen.getByText(/Compacted: 10 → 3 messages/i)).toBeInTheDocument();
    });
  });

  it('/model calls get_available_models and shows list', async () => {
    const user = userEvent.setup();
    setupDefaultInvokes({
      get_available_models: () => [
        { id: 'claude-sonnet-4-6', display_name: 'Sonnet 4.6', alias: 'sonnet', provider: 'Anthropic', tier: 'default', available: true },
      ],
    });
    await renderChat();

    await user.type(screen.getByPlaceholderText(/Message NexiBot/i), '/model');
    await user.keyboard('{Enter}');

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('get_available_models');
    });
    await waitFor(() => {
      expect(screen.getByText(/Current model/i)).toBeInTheDocument();
    });
  });

  it('/model opus calls set_session_model with opus shorthand', async () => {
    const user = userEvent.setup();
    setupDefaultInvokes({
      set_session_model: () => ({ model: 'claude-opus-4-6', thinking_budget: null, verbose: false, provider: null }),
    });
    await renderChat();

    await user.type(screen.getByPlaceholderText(/Message NexiBot/i), '/model opus');
    await user.keyboard('{Enter}');

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('set_session_model', { model: 'opus' });
    });
    await waitFor(() => {
      expect(screen.getByText(/Model switched to/i)).toBeInTheDocument();
    });
  });

  it('/model reset calls reset_session_overrides', async () => {
    const user = userEvent.setup();
    setupDefaultInvokes({
      reset_session_overrides: () => defaultOverrides(),
    });
    await renderChat();

    await user.type(screen.getByPlaceholderText(/Message NexiBot/i), '/model reset');
    await user.keyboard('{Enter}');

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('reset_session_overrides');
    });
  });

  it('/think calls toggle_thinking', async () => {
    const user = userEvent.setup();
    setupDefaultInvokes({
      toggle_thinking: () => ({ model: null, thinking_budget: 5000, verbose: false, provider: null }),
    });
    await renderChat();

    await user.type(screen.getByPlaceholderText(/Message NexiBot/i), '/think');
    await user.keyboard('{Enter}');

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('toggle_thinking', { budget: null });
    });
    await waitFor(() => {
      expect(screen.getByText(/Extended thinking.*enabled/i)).toBeInTheDocument();
    });
  });

  it('/verbose calls toggle_verbose', async () => {
    const user = userEvent.setup();
    setupDefaultInvokes({
      toggle_verbose: () => ({ model: null, thinking_budget: null, verbose: true, provider: null }),
    });
    await renderChat();

    await user.type(screen.getByPlaceholderText(/Message NexiBot/i), '/verbose');
    await user.keyboard('{Enter}');

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('toggle_verbose');
    });
    await waitFor(() => {
      expect(screen.getByText(/Verbose mode.*enabled/i)).toBeInTheDocument();
    });
  });

  it('/provider claude calls set_session_provider', async () => {
    const user = userEvent.setup();
    setupDefaultInvokes({
      set_session_provider: () => ({ model: null, thinking_budget: null, verbose: false, provider: 'claude' }),
    });
    await renderChat();

    await user.type(screen.getByPlaceholderText(/Message NexiBot/i), '/provider claude');
    await user.keyboard('{Enter}');

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('set_session_provider', { provider: 'claude' });
    });
  });

  it('/remind daily creates a scheduled task', async () => {
    const user = userEvent.setup();
    setupDefaultInvokes({
      add_scheduled_task: () => ({ id: 'task-1', name: 'Check calendar' }),
    });
    await renderChat();

    await user.type(screen.getByPlaceholderText(/Message NexiBot/i), '/remind daily 09:00 Check calendar');
    await user.keyboard('{Enter}');

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('add_scheduled_task', expect.objectContaining({
        schedule: 'daily 09:00',
        prompt: 'Check calendar',
      }));
    });
    await waitFor(() => {
      expect(screen.getByText(/Scheduled:.*Check calendar/i)).toBeInTheDocument();
    });
  });
});

// ─── Session overrides bar ────────────────────────────────────────────────────

describe('Session overrides bar', () => {
  it('hidden when no overrides and no context usage', async () => {
    await renderChat();
    expect(screen.queryByRole('button', { name: 'Reset' })).not.toBeInTheDocument();
  });

  it('shows model badge when model override is set', async () => {
    setupDefaultInvokes({
      get_session_overrides: () => ({ model: 'claude-opus-4-6', thinking_budget: null, verbose: false, provider: null }),
    });
    await renderChat();
    expect(screen.getByText('Opus 4.6')).toBeInTheDocument();
  });

  it('shows thinking budget badge when set', async () => {
    setupDefaultInvokes({
      get_session_overrides: () => ({ model: null, thinking_budget: 5000, verbose: false, provider: null }),
    });
    await renderChat();
    expect(screen.getByText(/Thinking: 5,000/i)).toBeInTheDocument();
  });

  it('shows Verbose badge when verbose is true', async () => {
    setupDefaultInvokes({
      get_session_overrides: () => ({ model: null, thinking_budget: null, verbose: true, provider: null }),
    });
    await renderChat();
    expect(screen.getByText('Verbose')).toBeInTheDocument();
  });

  it('shows provider badge when provider is set', async () => {
    setupDefaultInvokes({
      get_session_overrides: () => ({ model: null, thinking_budget: null, verbose: false, provider: 'claude' }),
    });
    await renderChat();
    expect(screen.getByText('claude')).toBeInTheDocument();
  });

  it('Reset button calls reset_session_overrides', async () => {
    const user = userEvent.setup();
    setupDefaultInvokes({
      get_session_overrides: () => ({ model: 'claude-opus-4-6', thinking_budget: null, verbose: false, provider: null }),
      reset_session_overrides: () => defaultOverrides(),
    });
    await renderChat();

    await user.click(screen.getByRole('button', { name: 'Reset' }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('reset_session_overrides');
    });
  });
});

// ─── Agent selector ───────────────────────────────────────────────────────────

describe('Agent selector', () => {
  it('is hidden when only 1 agent is available', async () => {
    setupDefaultInvokes({
      list_agents: () => [{ id: 'agent-1', name: 'NexiBot', model: null, is_default: true }],
    });
    await renderChat();
    expect(screen.queryByText('Agent:')).not.toBeInTheDocument();
  });

  it('is shown when 2+ agents are available', async () => {
    setupDefaultInvokes({
      list_agents: () => [
        { id: 'agent-1', name: 'NexiBot', model: null, is_default: true },
        { id: 'agent-2', name: 'Researcher', model: null, is_default: false },
      ],
      get_active_gui_agent: () => 'agent-1',
    });
    await renderChat();
    await waitFor(() => expect(screen.getByText('Agent:')).toBeInTheDocument());
    expect(screen.getByRole('option', { name: /NexiBot/ })).toBeInTheDocument();
    expect(screen.getByRole('option', { name: /Researcher/ })).toBeInTheDocument();
  });

  it('changing agent calls set_active_gui_agent', async () => {
    const user = userEvent.setup();
    setupDefaultInvokes({
      list_agents: () => [
        { id: 'agent-1', name: 'NexiBot', model: null, is_default: true },
        { id: 'agent-2', name: 'Researcher', model: null, is_default: false },
      ],
      get_active_gui_agent: () => 'agent-1',
    });
    await renderChat();
    await waitFor(() => screen.getByRole('combobox'));

    await user.selectOptions(screen.getByRole('combobox'), 'agent-2');

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('set_active_gui_agent', { agentId: 'agent-2' });
    });
  });
});

// ─── Voice bar ────────────────────────────────────────────────────────────────

describe('Voice bar', () => {
  it('"Start voice service" button exists when voice is off', async () => {
    await renderChat();
    expect(screen.getByRole('button', { name: 'Start voice service' })).toBeInTheDocument();
  });

  it('clicking Start Voice calls start_voice_service', async () => {
    const user = userEvent.setup();
    await renderChat();

    await user.click(screen.getByRole('button', { name: 'Start voice service' }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('start_voice_service');
    });
  });

  it('shows "Stop voice service" button when voice service is running', async () => {
    await renderChatWithVoice();
    expect(screen.getByRole('button', { name: 'Stop voice service' })).toBeInTheDocument();
  });

  it('clicking Stop calls stop_voice_service', async () => {
    const user = userEvent.setup();
    await renderChatWithVoice();

    await user.click(screen.getByRole('button', { name: 'Stop voice service' }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('stop_voice_service');
    });
  });

  it('TTS toggle (🔊) calls set_voice_response_enabled with false', async () => {
    const user = userEvent.setup();
    await renderChatWithVoice({ voice_response_enabled: true });

    const ttsBtn = screen.getByTitle(/Voice response on/i);
    await user.click(ttsBtn);

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('set_voice_response_enabled', { enabled: false });
    });
  });

  it('TTS toggle (🔇) calls set_voice_response_enabled with true', async () => {
    const user = userEvent.setup();
    await renderChatWithVoice({ voice_response_enabled: false });

    const ttsBtn = screen.getByTitle(/Voice response off/i);
    await user.click(ttsBtn);

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('set_voice_response_enabled', { enabled: true });
    });
  });

  it('wake word toggle calls voice_set_wakeword_enabled', async () => {
    const user = userEvent.setup();
    await renderChatWithVoice({ wakeword_enabled: false });

    const wakeBtn = screen.getByTitle(/Wake word off/i);
    await user.click(wakeBtn);

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('voice_set_wakeword_enabled', { enabled: true });
    });
  });

  it('force stop button visible when voice is active (Listening state)', async () => {
    await renderChatWithVoice({ state: 'Listening' });
    expect(screen.getByRole('button', { name: 'Stop listening and return to idle' })).toBeInTheDocument();
  });

  it('force stop button hidden when voice is idle', async () => {
    await renderChatWithVoice({ state: 'Idle' });
    expect(screen.queryByRole('button', { name: 'Stop listening and return to idle' })).not.toBeInTheDocument();
  });

  it('clicking force stop calls voice_stop_listening', async () => {
    const user = userEvent.setup();
    await renderChatWithVoice({ state: 'Listening' });

    await user.click(screen.getByRole('button', { name: 'Stop listening and return to idle' }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('voice_stop_listening');
    });
  });
});

// ─── PTT ─────────────────────────────────────────────────────────────────────

describe('Push-to-talk (PTT)', () => {
  it('mousedown on PTT button calls ptt_start', async () => {
    await renderChat();

    const pttBtn = screen.getByTitle('Hold to talk');
    pttBtn.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('ptt_start');
    });
  });

  it('shows waveform animation while recording', async () => {
    await renderChat();

    const pttBtn = screen.getByTitle('Hold to talk');
    pttBtn.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }));

    await waitFor(() => {
      expect(pttBtn.classList.contains('recording')).toBe(true);
    });
  });

  it('mouseleave calls ptt_cancel', async () => {
    await renderChat();

    const pttBtn = screen.getByTitle('Hold to talk');
    // Start recording first
    pttBtn.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }));
    await waitFor(() => expect(vi.mocked(invoke)).toHaveBeenCalledWith('ptt_start'));

    // Leave button — use fireEvent.mouseLeave so React's synthetic onMouseLeave fires
    fireEvent.mouseLeave(pttBtn);

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('ptt_cancel');
    });
  });

  it('mouseup calls ptt_stop and shows transcript and response', async () => {
    setupDefaultInvokes({
      ptt_stop: () => ({ transcript: 'What time is it?', response: 'It is 3pm.', error: undefined }),
    });
    await renderChat();

    const pttBtn = screen.getByTitle('Hold to talk');
    pttBtn.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }));
    await waitFor(() => expect(vi.mocked(invoke)).toHaveBeenCalledWith('ptt_start'));

    pttBtn.dispatchEvent(new MouseEvent('mouseup', { bubbles: true }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('ptt_stop');
    });
    await waitFor(() => {
      expect(screen.getByText('What time is it?')).toBeInTheDocument();
      expect(screen.getByText('It is 3pm.')).toBeInTheDocument();
    });
  });
});

// ─── Background tasks ─────────────────────────────────────────────────────────

describe('Background task pills', () => {
  it('task bar hidden when no tasks running', async () => {
    await renderChat();
    expect(screen.queryByText(/⏳/)).not.toBeInTheDocument();
  });

  it('task:started event shows a task pill', async () => {
    await renderChat();

    act(() => {
      dispatchTauriEvent('task:started', { id: 'task-1', description: 'Searching the web…', status: 'running' });
    });

    await waitFor(() => {
      expect(screen.getByText(/Searching the web/i)).toBeInTheDocument();
    });
  });

  it('task:complete event removes the task pill after delay', async () => {
    vi.useFakeTimers();
    try {
      render(<Chat />);
      await act(async () => { await Promise.resolve(); });
      act(() => {
        dispatchTauriEvent('task:started', { id: 'task-1', description: 'Running analysis', status: 'running' });
      });
      // With fake timers, check synchronously after act
      expect(screen.getByText(/Running analysis/i)).toBeInTheDocument();
      act(() => {
        dispatchTauriEvent('task:complete', { task_id: 'task-1', summary: 'Done.' });
      });
      // Advance fake time past the 5s removal delay
      await act(async () => { vi.advanceTimersByTime(6000); });
      // After advancing fake timers, the state update is flushed by act — check synchronously
      expect(screen.queryByText(/Running analysis/i)).not.toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });

  it('task:failed event clears the task pill after delay', async () => {
    vi.useFakeTimers();
    try {
      render(<Chat />);
      await act(async () => { await Promise.resolve(); });
      act(() => {
        dispatchTauriEvent('task:started', { id: 'task-2', description: 'Running analysis', status: 'running' });
      });
      expect(screen.getByText(/Running analysis/i)).toBeInTheDocument();
      act(() => {
        dispatchTauriEvent('task:failed', { task_id: 'task-2', error: 'Boom' });
      });
      expect(screen.queryByText(/Running analysis/i)).not.toBeInTheDocument();
      await act(async () => { vi.advanceTimersByTime(6000); });
      expect(screen.queryByText(/Running analysis/i)).not.toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });
});

// ─── Tauri event listeners ────────────────────────────────────────────────────

describe('Tauri event: voice:transcript', () => {
  it('voice:transcript adds a user message with 🎙️ prefix for user role', async () => {
    await renderChat();

    act(() => {
      dispatchTauriEvent('voice:transcript', { role: 'user', content: 'Hey Nexus!' });
    });

    await waitFor(() => {
      expect(screen.getByText('🎙️ Hey Nexus!')).toBeInTheDocument();
    });
  });

  it('voice:transcript adds an assistant message for assistant role', async () => {
    await renderChat();

    act(() => {
      dispatchTauriEvent('voice:transcript', { role: 'assistant', content: 'How can I help?' });
    });

    await waitFor(() => {
      expect(screen.getByText('How can I help?')).toBeInTheDocument();
    });
  });
});

describe('Tauri event: compact:status', () => {
  it('auto_compacting event adds a system message', async () => {
    await renderChat();

    act(() => {
      dispatchTauriEvent('compact:status', { status: 'auto_compacting', message: 'Compacting…' });
    });

    await waitFor(() => {
      expect(screen.getByText(/\[Auto-compact\].*Compacting/i)).toBeInTheDocument();
    });
  });
});

// ─── GuardrailsPanel integration ─────────────────────────────────────────────

describe('GuardrailsPanel integration', () => {
  it('/guardrails command shows GuardrailsPanel', async () => {
    const user = userEvent.setup();
    await renderChat();

    await user.type(screen.getByPlaceholderText(/Message NexiBot/i), '/guardrails');
    await user.keyboard('{Enter}');

    expect(screen.getByTestId('guardrails-panel')).toBeInTheDocument();
  });

  it('closing GuardrailsPanel hides it', async () => {
    const user = userEvent.setup();
    await renderChat();

    await user.type(screen.getByPlaceholderText(/Message NexiBot/i), '/guardrails');
    await user.keyboard('{Enter}');
    await user.click(screen.getByRole('button', { name: 'Close Guardrails' }));

    expect(screen.queryByTestId('guardrails-panel')).not.toBeInTheDocument();
  });

  it('applying guardrails shows confirmation message and closes panel', async () => {
    const user = userEvent.setup();
    await renderChat();

    await user.type(screen.getByPlaceholderText(/Message NexiBot/i), '/guardrails');
    await user.keyboard('{Enter}');
    await user.click(screen.getByRole('button', { name: 'Apply Guardrails' }));

    expect(screen.queryByTestId('guardrails-panel')).not.toBeInTheDocument();
    expect(screen.getByText(/Guardrails configuration updated/i)).toBeInTheDocument();
  });
});
