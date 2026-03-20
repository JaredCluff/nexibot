import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { KnowledgeTab } from './KnowledgeTab';
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
// invoke is mocked globally via setup.ts

// -------------------------------------------------------------------
// Fixture helpers
// -------------------------------------------------------------------
const makeConfig = (overrides: Record<string, any> = {}) => ({
  k2k: {
    enabled: false,
    local_agent_url: 'ws://localhost:8765',
    router_url: '',
    client_id: 'test-client-id',
    supermemory_enabled: false,
    supermemory_auto_extract: false,
  },
  ...overrides,
});

function makeMemory(overrides: Partial<{
  id: string;
  content: string;
  memory_type: string;
  tags: string[];
  created_at: string;
  last_accessed: string;
  access_count: number;
}> = {}) {
  return {
    id: overrides.id ?? 'mem-1',
    content: overrides.content ?? 'Important fact',
    memory_type: overrides.memory_type ?? 'Fact',
    tags: overrides.tags ?? [],
    created_at: overrides.created_at ?? '2024-01-01T00:00:00Z',
    last_accessed: overrides.last_accessed ?? '2024-01-01T00:00:00Z',
    access_count: overrides.access_count ?? 0,
  };
}

function makeK2kResult(overrides: Partial<{
  title: string;
  source_type: string;
  confidence: number;
  summary: string;
  content: string;
}> = {}) {
  return {
    title: overrides.title ?? 'Test Result',
    source_type: overrides.source_type ?? 'local',
    confidence: overrides.confidence ?? 0.9,
    summary: overrides.summary ?? 'A test result summary',
    content: overrides.content ?? 'Full content here.',
  };
}

function setupSettings(overrides: {
  config?: Record<string, any>;
  supermemoryAvailable?: boolean | null;
  checkingSupermemory?: boolean;
  settings?: Record<string, any>;
} = {}) {
  const config = makeConfig(overrides.config ?? {});
  const setConfig = vi.fn();
  const checkSupermemory = vi.fn();

  vi.mocked(useSettings).mockReturnValue({
    config,
    setConfig,
    supermemoryAvailable: overrides.supermemoryAvailable ?? false,
    checkSupermemory,
    checkingSupermemory: overrides.checkingSupermemory ?? false,
    ...(overrides.settings ?? {}),
  } as ReturnType<typeof useSettings>);

  return { config, setConfig, checkSupermemory };
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
    vi.mocked(useSettings).mockReturnValue({
      config: null,
      setConfig: vi.fn(),
      supermemoryAvailable: false,
      checkSupermemory: vi.fn(),
      checkingSupermemory: false,
    } as ReturnType<typeof useSettings>);

    const { container } = render(<KnowledgeTab />);
    expect(container.firstChild).toBeNull();
  });

  it('renders K2K Integration and Memory Browser headings', () => {
    setupSettings();
    render(<KnowledgeTab />);
    expect(screen.getByText('K2K Integration')).toBeInTheDocument();
    expect(screen.getByText('Memory Browser')).toBeInTheDocument();
  });
});

// ===================================================================
// 2. K2K Integration
// ===================================================================
describe('K2K Integration', () => {
  it('enable K2K checkbox calls setConfig with enabled=true', async () => {
    const user = userEvent.setup();
    const { setConfig, config } = setupSettings();

    render(<KnowledgeTab />);
    await user.click(screen.getByRole('checkbox', { name: /Enable K2K/i }));

    expect(setConfig).toHaveBeenCalledWith({
      ...config,
      k2k: { ...config.k2k, enabled: true },
    });
  });

  it('K2K details are hidden when K2K is disabled', () => {
    setupSettings({
      config: { k2k: { enabled: false, local_agent_url: 'ws://localhost:8765', router_url: '', client_id: 'test-client-id', supermemory_enabled: false, supermemory_auto_extract: false } },
    });

    render(<KnowledgeTab />);
    expect(screen.queryByText('Local Agent URL')).not.toBeInTheDocument();
    expect(screen.queryByText('Router URL (optional)')).not.toBeInTheDocument();
  });

  it('local_agent_url input is shown when K2K is enabled', () => {
    setupSettings({
      config: { k2k: { enabled: true, local_agent_url: 'ws://localhost:8765', router_url: '', client_id: 'test-client-id', supermemory_enabled: false, supermemory_auto_extract: false } },
    });

    render(<KnowledgeTab />);
    expect(screen.getByText('Local Agent URL')).toBeInTheDocument();
    expect(screen.getByDisplayValue('ws://localhost:8765')).toBeInTheDocument();
  });

  it('router_url input is shown when K2K is enabled', () => {
    setupSettings({
      config: { k2k: { enabled: true, local_agent_url: 'ws://localhost:8765', router_url: 'http://router.example.com', client_id: 'test-client-id', supermemory_enabled: false, supermemory_auto_extract: false } },
    });

    render(<KnowledgeTab />);
    expect(screen.getByText(/Router URL/i)).toBeInTheDocument();
    expect(screen.getByDisplayValue('http://router.example.com')).toBeInTheDocument();
  });
});

// ===================================================================
// 3. Supermemory status
// ===================================================================
describe('Supermemory status', () => {
  it('shows "System Agent connected" when supermemoryAvailable is true', () => {
    setupSettings({ supermemoryAvailable: true });
    render(<KnowledgeTab />);
    expect(screen.getByText('System Agent connected')).toBeInTheDocument();
  });

  it('shows "System Agent not detected" when supermemoryAvailable is false', () => {
    setupSettings({ supermemoryAvailable: false });
    render(<KnowledgeTab />);
    expect(screen.getByText('System Agent not detected')).toBeInTheDocument();
  });

  it('Check Status button calls checkSupermemory', async () => {
    const user = userEvent.setup();
    const { checkSupermemory } = setupSettings();
    render(<KnowledgeTab />);

    await user.click(screen.getByRole('button', { name: 'Check Status' }));
    expect(checkSupermemory).toHaveBeenCalled();
  });
});

// ===================================================================
// 4. Supermemory toggles
// ===================================================================
describe('Supermemory toggles', () => {
  it('supermemory_enabled checkbox calls setConfig', async () => {
    const user = userEvent.setup();
    const { setConfig, config } = setupSettings();

    render(<KnowledgeTab />);
    await user.click(screen.getByRole('checkbox', { name: /Enable Supermemory/i }));

    expect(setConfig).toHaveBeenCalledWith({
      ...config,
      k2k: { ...config.k2k, supermemory_enabled: true },
    });
  });

  it('auto_extract checkbox is hidden when supermemory_enabled is false', () => {
    setupSettings({
      config: { k2k: { enabled: false, local_agent_url: 'ws://localhost:8765', router_url: '', client_id: 'test-client-id', supermemory_enabled: false, supermemory_auto_extract: false } },
    });

    render(<KnowledgeTab />);
    expect(screen.queryByRole('checkbox', { name: /Auto-extract knowledge/i })).not.toBeInTheDocument();
  });

  it('auto_extract checkbox is shown when supermemory_enabled is true', () => {
    setupSettings({
      config: { k2k: { enabled: false, local_agent_url: 'ws://localhost:8765', router_url: '', client_id: 'test-client-id', supermemory_enabled: true, supermemory_auto_extract: false } },
    });

    render(<KnowledgeTab />);
    expect(screen.getByRole('checkbox', { name: /Auto-extract knowledge/i })).toBeInTheDocument();
  });
});

// ===================================================================
// 5. Memory search
// ===================================================================
describe('Memory search', () => {
  it('Search button calls search_memories with query', async () => {
    const user = userEvent.setup();
    setupSettings();
    vi.mocked(invoke).mockResolvedValue([]);

    render(<KnowledgeTab />);
    const searchInput = screen.getByPlaceholderText('Search memories...');
    fireEvent.change(searchInput, { target: { value: 'my query' } });

    await user.click(screen.getByRole('button', { name: 'Search' }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('search_memories', { query: 'my query' });
    });
  });

  it('pressing Enter in search input calls search_memories', async () => {
    const user = userEvent.setup();
    setupSettings();
    vi.mocked(invoke).mockResolvedValue([]);

    render(<KnowledgeTab />);
    const searchInput = screen.getByPlaceholderText('Search memories...');
    await user.click(searchInput);
    fireEvent.change(searchInput, { target: { value: 'enter search' } });
    await user.keyboard('{Enter}');

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('search_memories', { query: 'enter search' });
    });
  });

  it('shows result count after search', async () => {
    const user = userEvent.setup();
    setupSettings();
    vi.mocked(invoke).mockResolvedValue([makeMemory(), makeMemory({ id: 'mem-2' })]);

    render(<KnowledgeTab />);
    fireEvent.change(screen.getByPlaceholderText('Search memories...'), { target: { value: 'test' } });
    await user.click(screen.getByRole('button', { name: 'Search' }));

    await waitFor(() => {
      expect(screen.getByText(/2 results/i)).toBeInTheDocument();
    });
  });

  it('shows memory cards with content after search', async () => {
    const user = userEvent.setup();
    setupSettings();
    vi.mocked(invoke).mockResolvedValue([makeMemory({ content: 'Very important fact' })]);

    render(<KnowledgeTab />);
    fireEvent.change(screen.getByPlaceholderText('Search memories...'), { target: { value: 'important' } });
    await user.click(screen.getByRole('button', { name: 'Search' }));

    await waitFor(() => {
      expect(screen.getByText('Very important fact')).toBeInTheDocument();
    });
  });
});

// ===================================================================
// 6. Memory filter
// ===================================================================
describe('Memory filter', () => {
  it('changing type filter to Fact and searching calls get_memories_by_type', async () => {
    const user = userEvent.setup();
    setupSettings();
    vi.mocked(invoke).mockResolvedValue([]);

    render(<KnowledgeTab />);
    const typeSelect = screen.getByRole('combobox');
    fireEvent.change(typeSelect, { target: { value: 'Fact' } });

    await user.click(screen.getByRole('button', { name: 'Search' }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('get_memories_by_type', { memoryType: 'Fact' });
    });
  });

  it('All type filter uses search_memories', async () => {
    const user = userEvent.setup();
    setupSettings();
    vi.mocked(invoke).mockResolvedValue([]);

    render(<KnowledgeTab />);
    // type filter defaults to All
    fireEvent.change(screen.getByPlaceholderText('Search memories...'), { target: { value: 'something' } });
    await user.click(screen.getByRole('button', { name: 'Search' }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('search_memories', { query: 'something' });
    });
  });
});

// ===================================================================
// 7. Memory delete
// ===================================================================
describe('Memory delete', () => {
  async function renderWithMemory() {
    const user = userEvent.setup();
    setupSettings();
    vi.mocked(invoke).mockResolvedValue([makeMemory({ id: 'mem-del', content: 'Delete me' })]);

    render(<KnowledgeTab />);
    fireEvent.change(screen.getByPlaceholderText('Search memories...'), { target: { value: 'delete' } });
    await user.click(screen.getByRole('button', { name: 'Search' }));
    await waitFor(() => screen.getByText('Delete me'));
    return user;
  }

  it('Delete button calls confirm and delete_memory', async () => {
    mockConfirm.mockResolvedValue(true);
    const user = await renderWithMemory();

    vi.mocked(invoke).mockResolvedValue(undefined);
    await user.click(screen.getByRole('button', { name: 'Delete' }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('delete_memory', { memoryId: 'mem-del' });
    });
  });

  it('does not delete memory if confirm is cancelled', async () => {
    mockConfirm.mockResolvedValue(false);
    const user = await renderWithMemory();

    await user.click(screen.getByRole('button', { name: 'Delete' }));

    expect(vi.mocked(invoke)).not.toHaveBeenCalledWith('delete_memory', expect.anything());
  });
});

// ===================================================================
// 8. Add memory form
// ===================================================================
describe('Add memory form', () => {
  it('shows add form when "+ Add Memory" button is clicked', async () => {
    const user = userEvent.setup();
    setupSettings();
    render(<KnowledgeTab />);

    await user.click(screen.getByRole('button', { name: '+ Add Memory' }));

    expect(screen.getByPlaceholderText(/What should NexiBot remember/i)).toBeInTheDocument();
  });

  it('Add Memory button is disabled when content is empty', async () => {
    const user = userEvent.setup();
    setupSettings();
    render(<KnowledgeTab />);

    await user.click(screen.getByRole('button', { name: '+ Add Memory' }));

    expect(screen.getByRole('button', { name: /Add Memory/i })).toBeDisabled();
  });

  it('adds memory via invoke add_memory with correct args', async () => {
    const user = userEvent.setup();
    setupSettings();
    vi.mocked(invoke).mockResolvedValue([]);

    render(<KnowledgeTab />);
    await user.click(screen.getByRole('button', { name: '+ Add Memory' }));

    const contentArea = screen.getByPlaceholderText(/What should NexiBot remember/i);
    fireEvent.change(contentArea, { target: { value: 'My new memory content' } });

    // Set tags
    const tagsInput = screen.getByPlaceholderText('tag1, tag2, tag3');
    fireEvent.change(tagsInput, { target: { value: 'work, notes' } });

    await user.click(screen.getByRole('button', { name: /Add Memory/i }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('add_memory', {
        content: 'My new memory content',
        memoryType: 'Fact',
        tags: ['work', 'notes'],
      });
    });
  });

  it('Cancel button hides the add form', async () => {
    const user = userEvent.setup();
    setupSettings();
    render(<KnowledgeTab />);

    await user.click(screen.getByRole('button', { name: '+ Add Memory' }));
    expect(screen.getByPlaceholderText(/What should NexiBot remember/i)).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'Cancel' }));

    expect(screen.queryByPlaceholderText(/What should NexiBot remember/i)).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: '+ Add Memory' })).toBeInTheDocument();
  });
});

// ===================================================================
// 9. K2K search
// ===================================================================
describe('K2K search', () => {
  const enabledK2kConfig = {
    k2k: {
      enabled: true,
      local_agent_url: 'ws://localhost:8765',
      router_url: '',
      client_id: 'test-client-id',
      supermemory_enabled: false,
      supermemory_auto_extract: false,
    },
  };

  it('K2K search query input is present when K2K is enabled', () => {
    setupSettings({ config: enabledK2kConfig });
    render(<KnowledgeTab />);
    expect(screen.getByPlaceholderText('Search knowledge...')).toBeInTheDocument();
  });

  it('Search button calls invoke search_k2k with query', async () => {
    const user = userEvent.setup();
    setupSettings({ config: enabledK2kConfig });
    vi.mocked(invoke).mockResolvedValue([]);

    render(<KnowledgeTab />);
    const k2kInput = screen.getByPlaceholderText('Search knowledge...');
    fireEvent.change(k2kInput, { target: { value: 'k2k test query' } });

    // There will be multiple Search buttons — the K2K one
    const searchButtons = screen.getAllByRole('button', { name: 'Search' });
    // The second Search button belongs to K2K (Memory Search is first, K2K is second)
    await user.click(searchButtons[searchButtons.length - 1]);

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('search_k2k', expect.objectContaining({
        query: 'k2k test query',
      }));
    });
  });

  it('K2K search results are shown after search', async () => {
    const user = userEvent.setup();
    setupSettings({ config: enabledK2kConfig });
    vi.mocked(invoke).mockResolvedValue([makeK2kResult({ title: 'K2K Result Title', summary: 'A great summary' })]);

    render(<KnowledgeTab />);
    const k2kInput = screen.getByPlaceholderText('Search knowledge...');
    fireEvent.change(k2kInput, { target: { value: 'result query' } });

    const searchButtons = screen.getAllByRole('button', { name: 'Search' });
    await user.click(searchButtons[searchButtons.length - 1]);

    await waitFor(() => {
      expect(screen.getByText('K2K Result Title')).toBeInTheDocument();
      expect(screen.getByText('A great summary')).toBeInTheDocument();
    });
  });
});

// ===================================================================
// 10. Agent capabilities
// ===================================================================
describe('Agent capabilities', () => {
  const enabledK2kConfig = {
    k2k: {
      enabled: true,
      local_agent_url: 'ws://localhost:8765',
      router_url: '',
      client_id: 'test-client-id',
      supermemory_enabled: false,
      supermemory_auto_extract: false,
    },
  };

  it('Load Capabilities button calls get_agent_capabilities', async () => {
    const user = userEvent.setup();
    setupSettings({ config: enabledK2kConfig });
    vi.mocked(invoke).mockResolvedValue([]);

    render(<KnowledgeTab />);
    await user.click(screen.getByRole('button', { name: 'Load Capabilities' }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('get_agent_capabilities');
    });
  });

  it('shows capability select after Load Capabilities returns results', async () => {
    const user = userEvent.setup();
    setupSettings({ config: enabledK2kConfig });
    vi.mocked(invoke).mockResolvedValue(['cap-alpha', 'cap-beta']);

    render(<KnowledgeTab />);
    await user.click(screen.getByRole('button', { name: 'Load Capabilities' }));

    await waitFor(() => {
      expect(screen.getByText('Capability')).toBeInTheDocument();
      expect(screen.getByDisplayValue('cap-alpha')).toBeInTheDocument();
    });
  });

  it('Submit Task button calls submit_agent_task with selected capability', async () => {
    const user = userEvent.setup();
    setupSettings({ config: enabledK2kConfig });
    vi.mocked(invoke)
      .mockResolvedValueOnce(['cap-alpha', 'cap-beta']) // get_agent_capabilities
      .mockResolvedValueOnce({ task_id: 'new-task-id' }); // submit_agent_task

    render(<KnowledgeTab />);
    await user.click(screen.getByRole('button', { name: 'Load Capabilities' }));
    await waitFor(() => screen.getByDisplayValue('cap-alpha'));

    await user.click(screen.getByRole('button', { name: 'Submit Task' }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('submit_agent_task', expect.objectContaining({
        capability: 'cap-alpha',
      }));
    });
  });
});
