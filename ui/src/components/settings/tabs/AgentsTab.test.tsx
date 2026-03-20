import React from 'react';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { AgentsTab } from './AgentsTab';
import { invoke } from '@tauri-apps/api/core';
import { useSettings } from '../SettingsContext';

// -------------------------------------------------------------------
// Mocks
// -------------------------------------------------------------------
const mockConfirm = vi.hoisted(() => vi.fn(() => Promise.resolve(true)));
vi.mock('../SettingsContext');
vi.mock('../shared/InfoTip', () => ({ InfoTip: () => null }));
vi.mock('../../../shared/useConfirm', () => ({
  useConfirm: () => ({ confirm: mockConfirm, modal: null }),
}));

// -------------------------------------------------------------------
// Fixture helpers
// -------------------------------------------------------------------
function makeAgent(overrides: Partial<{
  id: string;
  name: string;
  model: string | null;
  provider: string | null;
  is_default: boolean;
  channel_bindings: { channel: string; peer_id: string | null }[];
}> = {}) {
  return {
    id: overrides.id ?? 'agent-1',
    name: overrides.name ?? 'Agent One',
    model: overrides.model !== undefined ? overrides.model : 'claude-opus-4-6',
    provider: overrides.provider !== undefined ? overrides.provider : null,
    is_default: overrides.is_default ?? false,
    channel_bindings: overrides.channel_bindings ?? [],
  };
}

function makeSession(overrides: Partial<{
  id: string;
  name: string;
  created_at: string;
}> = {}) {
  return {
    id: overrides.id ?? 'session-1',
    name: overrides.name ?? 'Session One',
    created_at: overrides.created_at ?? '2024-01-01T00:00:00Z',
  };
}

function makeMessage(overrides: Partial<{
  from_session: string;
  to_session: string;
  content: string;
  timestamp: string;
}> = {}) {
  return {
    from_session: overrides.from_session ?? 'session-1',
    to_session: overrides.to_session ?? 'session-2',
    content: overrides.content ?? 'Hello from session 1',
    timestamp: overrides.timestamp ?? '2024-01-01T00:00:00Z',
  };
}

function setupSettings(overrides: {
  agents?: ReturnType<typeof makeAgent>[];
  activeGuiAgent?: string;
  setActiveGuiAgent?: ReturnType<typeof vi.fn>;
} = {}) {
  const setActiveGuiAgent = overrides.setActiveGuiAgent ?? vi.fn();

  vi.mocked(useSettings).mockReturnValue({
    agents: overrides.agents ?? [],
    activeGuiAgent: overrides.activeGuiAgent ?? '',
    setActiveGuiAgent,
  } as ReturnType<typeof useSettings>);

  return { setActiveGuiAgent };
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
// 1. Agents section — no agents
// ===================================================================
describe('Agents section — no agents', () => {
  it('shows "No agents configured. Using default agent." when agents is empty', () => {
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_named_sessions') return [];
      return undefined;
    });
    setupSettings({ agents: [] });
    render(<AgentsTab />);
    expect(screen.getByText(/No agents configured\. Using default agent\./i)).toBeInTheDocument();
  });

  it('does not show an agent select when agents is empty', () => {
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_named_sessions') return [];
      return undefined;
    });
    setupSettings({ agents: [] });
    render(<AgentsTab />);
    // No select element for Active GUI Agent
    expect(screen.queryByText('Active GUI Agent')).not.toBeInTheDocument();
  });
});

// ===================================================================
// 2. Agents section — with agents
// ===================================================================
describe('Agents section — with agents', () => {
  it('shows agent select when agents are present', () => {
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_named_sessions') return [];
      return undefined;
    });
    setupSettings({ agents: [makeAgent({ id: 'a1', name: 'Agent One' })], activeGuiAgent: 'a1' });
    render(<AgentsTab />);
    expect(screen.getByText('Active GUI Agent')).toBeInTheDocument();
    expect(screen.getByRole('combobox')).toBeInTheDocument();
  });

  it('shows agent names in the select options', () => {
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_named_sessions') return [];
      return undefined;
    });
    setupSettings({
      agents: [
        makeAgent({ id: 'a1', name: 'Agent Alpha' }),
        makeAgent({ id: 'a2', name: 'Agent Beta' }),
      ],
      activeGuiAgent: 'a1',
    });
    render(<AgentsTab />);
    expect(screen.getByRole('option', { name: 'Agent Alpha' })).toBeInTheDocument();
    expect(screen.getByRole('option', { name: 'Agent Beta' })).toBeInTheDocument();
  });

  it('changing select calls set_active_gui_agent and setActiveGuiAgent', async () => {
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_named_sessions') return [];
      return undefined;
    });
    const { setActiveGuiAgent } = setupSettings({
      agents: [
        makeAgent({ id: 'a1', name: 'Agent Alpha' }),
        makeAgent({ id: 'a2', name: 'Agent Beta' }),
      ],
      activeGuiAgent: 'a1',
    });
    render(<AgentsTab />);

    const select = screen.getByRole('combobox');
    fireEvent.change(select, { target: { value: 'a2' } });

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('set_active_gui_agent', { agentId: 'a2' });
    });
    // setActiveGuiAgent is called after the async invoke resolves
    await waitFor(() => {
      expect(setActiveGuiAgent).toHaveBeenCalled();
    });
  });

  it('agent cards show name and model', () => {
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_named_sessions') return [];
      return undefined;
    });
    setupSettings({
      agents: [makeAgent({ id: 'a1', name: 'My Agent', model: 'claude-opus-4-6' })],
      activeGuiAgent: 'a1',
    });
    render(<AgentsTab />);
    expect(screen.getAllByText('My Agent').length).toBeGreaterThan(0);
    expect(screen.getByText('claude-opus-4-6')).toBeInTheDocument();
  });
});

// ===================================================================
// 3. Sessions — on mount
// ===================================================================
describe('Sessions — on mount', () => {
  it('calls list_named_sessions on mount', async () => {
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_named_sessions') return [];
      return undefined;
    });
    setupSettings();
    render(<AgentsTab />);

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('list_named_sessions');
    });
  });
});

// ===================================================================
// 4. Sessions — empty state
// ===================================================================
describe('Sessions — empty state', () => {
  it('shows "No named sessions. Create one to get started." when sessions is empty', async () => {
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_named_sessions') return [];
      return undefined;
    });
    setupSettings();
    render(<AgentsTab />);

    await waitFor(() => {
      expect(screen.getByText(/No named sessions\. Create one to get started\./i)).toBeInTheDocument();
    });
  });
});

// ===================================================================
// 5. Sessions — create form
// ===================================================================
describe('Sessions — create form', () => {
  it('clicking "+ Create Session" shows the create form', async () => {
    const user = userEvent.setup();
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_named_sessions') return [];
      return undefined;
    });
    setupSettings();
    render(<AgentsTab />);

    await user.click(screen.getByRole('button', { name: '+ Create Session' }));

    expect(screen.getByPlaceholderText(/Session name/i)).toBeInTheDocument();
  });

  it('Create button is disabled when session name is empty', async () => {
    const user = userEvent.setup();
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_named_sessions') return [];
      return undefined;
    });
    setupSettings();
    render(<AgentsTab />);

    await user.click(screen.getByRole('button', { name: '+ Create Session' }));

    expect(screen.getByRole('button', { name: 'Create' })).toBeDisabled();
  });

  it('Create calls create_named_session and hides form', async () => {
    const user = userEvent.setup();
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_named_sessions') return [];
      return undefined;
    });
    setupSettings();
    render(<AgentsTab />);

    await user.click(screen.getByRole('button', { name: '+ Create Session' }));
    const nameInput = screen.getByPlaceholderText(/Session name/i);
    await user.type(nameInput, 'My New Session');

    await user.click(screen.getByRole('button', { name: 'Create' }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('create_named_session', { name: 'My New Session' });
    });
    // Form should be hidden after creation
    expect(screen.queryByPlaceholderText(/Session name/i)).not.toBeInTheDocument();
  });

  it('Cancel hides the create form', async () => {
    const user = userEvent.setup();
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_named_sessions') return [];
      return undefined;
    });
    setupSettings();
    render(<AgentsTab />);

    await user.click(screen.getByRole('button', { name: '+ Create Session' }));
    expect(screen.getByPlaceholderText(/Session name/i)).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'Cancel' }));

    expect(screen.queryByPlaceholderText(/Session name/i)).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: '+ Create Session' })).toBeInTheDocument();
  });
});

// ===================================================================
// 6. Sessions — existing sessions
// ===================================================================
describe('Sessions — existing sessions', () => {
  it('shows session name', async () => {
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_named_sessions') return [makeSession({ id: 's1', name: 'Alpha Session' })];
      return undefined;
    });
    setupSettings();
    render(<AgentsTab />);

    await waitFor(() => {
      expect(screen.getByText('Alpha Session')).toBeInTheDocument();
    });
  });

  it('Switch To button calls switch_named_session', async () => {
    const user = userEvent.setup();
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_named_sessions') return [makeSession({ id: 's1', name: 'Alpha Session' })];
      return undefined;
    });
    setupSettings();
    render(<AgentsTab />);

    await waitFor(() => screen.getByText('Alpha Session'));

    await user.click(screen.getByRole('button', { name: 'Switch To' }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('switch_named_session', { sessionId: 's1' });
    });
  });

  it('View Inbox button calls get_session_inbox and shows messages', async () => {
    const user = userEvent.setup();
    const messages = [makeMessage({ content: 'Hello inbox!', from_session: 's2', to_session: 's1' })];
    vi.mocked(invoke).mockImplementation(async (cmd: string, args?: any) => {
      if (cmd === 'list_named_sessions') return [makeSession({ id: 's1', name: 'Alpha Session' })];
      if (cmd === 'get_session_inbox') return messages;
      return undefined;
    });
    setupSettings();
    render(<AgentsTab />);

    await waitFor(() => screen.getByText('Alpha Session'));

    await user.click(screen.getByRole('button', { name: 'View Inbox' }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('get_session_inbox', { sessionId: 's1' });
    });
    await waitFor(() => {
      expect(screen.getByText('Hello inbox!')).toBeInTheDocument();
    });
  });

  it('Delete button calls confirm and delete_named_session', async () => {
    const user = userEvent.setup();
    mockConfirm.mockResolvedValue(true);
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_named_sessions') return [makeSession({ id: 's1', name: 'Alpha Session' })];
      return undefined;
    });
    setupSettings();
    render(<AgentsTab />);

    await waitFor(() => screen.getByText('Alpha Session'));

    await user.click(screen.getByRole('button', { name: 'Delete' }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('delete_named_session', { sessionId: 's1' });
    });
  });
});

// ===================================================================
// 7. Sessions — inbox
// ===================================================================
describe('Sessions — inbox', () => {
  it('shows inbox messages when View Inbox is clicked', async () => {
    const user = userEvent.setup();
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_named_sessions') return [makeSession({ id: 's1', name: 'Alpha Session' })];
      if (cmd === 'get_session_inbox') return [
        makeMessage({ content: 'Message content here', from_session: 's2', to_session: 's1' }),
      ];
      return undefined;
    });
    setupSettings();
    render(<AgentsTab />);

    await waitFor(() => screen.getByText('Alpha Session'));
    await user.click(screen.getByRole('button', { name: 'View Inbox' }));

    await waitFor(() => {
      expect(screen.getByText('Message content here')).toBeInTheDocument();
    });
  });

  it('shows "Inbox is empty." when inbox has no messages', async () => {
    const user = userEvent.setup();
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_named_sessions') return [makeSession({ id: 's1', name: 'Alpha Session' })];
      if (cmd === 'get_session_inbox') return [];
      return undefined;
    });
    setupSettings();
    render(<AgentsTab />);

    await waitFor(() => screen.getByText('Alpha Session'));
    await user.click(screen.getByRole('button', { name: 'View Inbox' }));

    await waitFor(() => {
      expect(screen.getByText(/Inbox is empty\./i)).toBeInTheDocument();
    });
  });

  it('clicking "Hide Inbox" collapses the inbox', async () => {
    const user = userEvent.setup();
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_named_sessions') return [makeSession({ id: 's1', name: 'Alpha Session' })];
      if (cmd === 'get_session_inbox') return [];
      return undefined;
    });
    setupSettings();
    render(<AgentsTab />);

    await waitFor(() => screen.getByText('Alpha Session'));

    // Open inbox
    await user.click(screen.getByRole('button', { name: 'View Inbox' }));
    await waitFor(() => screen.getByRole('button', { name: 'Hide Inbox' }));

    // Close inbox
    await user.click(screen.getByRole('button', { name: 'Hide Inbox' }));

    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'View Inbox' })).toBeInTheDocument();
    });
    expect(screen.queryByText(/Inbox is empty\./i)).not.toBeInTheDocument();
  });
});

// ===================================================================
// 8. Inter-Agent Messaging
// ===================================================================
describe('Inter-Agent Messaging', () => {
  it('Inter-Agent Messaging section is hidden when fewer than 2 sessions', async () => {
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_named_sessions') return [makeSession({ id: 's1', name: 'Only Session' })];
      return undefined;
    });
    setupSettings();
    render(<AgentsTab />);

    await waitFor(() => screen.getByText('Only Session'));
    expect(screen.queryByText('Inter-Agent Messaging')).not.toBeInTheDocument();
  });

  it('Inter-Agent Messaging section is shown when 2 or more sessions exist', async () => {
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_named_sessions') return [
        makeSession({ id: 's1', name: 'Session One' }),
        makeSession({ id: 's2', name: 'Session Two' }),
      ];
      return undefined;
    });
    setupSettings();
    render(<AgentsTab />);

    await waitFor(() => {
      expect(screen.getByText('Inter-Agent Messaging')).toBeInTheDocument();
    });
  });

  it('Send Message button is disabled when from/to/content fields are empty', async () => {
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_named_sessions') return [
        makeSession({ id: 's1', name: 'Session One' }),
        makeSession({ id: 's2', name: 'Session Two' }),
      ];
      return undefined;
    });
    setupSettings();
    render(<AgentsTab />);

    await waitFor(() => screen.getByText('Inter-Agent Messaging'));

    expect(screen.getByRole('button', { name: 'Send Message' })).toBeDisabled();
  });

  it('Send Message calls send_inter_session_message with correct args', async () => {
    const user = userEvent.setup();
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_named_sessions') return [
        makeSession({ id: 's1', name: 'Session One' }),
        makeSession({ id: 's2', name: 'Session Two' }),
      ];
      return undefined;
    });
    setupSettings();
    render(<AgentsTab />);

    await waitFor(() => screen.getByText('Inter-Agent Messaging'));

    // Select From Session
    const fromSelects = screen.getAllByRole('combobox');
    // The first combobox after the agent select (if any) will be the From session
    // Find by label text "From Session"
    const fromLabel = screen.getByText('From Session');
    const fromSelect = fromLabel.closest('label')!.querySelector('select') as HTMLSelectElement;
    await user.selectOptions(fromSelect, 's1');

    // Select To Session
    const toLabel = screen.getByText('To Session');
    const toSelect = toLabel.closest('label')!.querySelector('select') as HTMLSelectElement;
    await user.selectOptions(toSelect, 's2');

    // Type message
    const textarea = screen.getByPlaceholderText('Message content...');
    await user.type(textarea, 'Hello from s1 to s2');

    await user.click(screen.getByRole('button', { name: 'Send Message' }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('send_inter_session_message', {
        fromSession: 's1',
        toSession: 's2',
        content: 'Hello from s1 to s2',
      });
    });
  });
});
