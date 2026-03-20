import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { AutomationTab } from './AutomationTab';
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
  webhooks: {
    enabled: false,
    port: 18791,
    auth_token: undefined as string | undefined,
    endpoints: [] as any[],
    tls: { enabled: false, auto_generate: true },
    rate_limit: { max_attempts: 10, window_secs: 60, lockout_secs: 300 },
  },
  ...overrides,
});

function makeTask(overrides: Partial<{
  id: string;
  name: string;
  schedule: string;
  prompt: string;
  enabled: boolean;
  last_run: string | null;
  run_if_missed: boolean;
}> = {}) {
  return {
    id: overrides.id ?? 'task-1',
    name: overrides.name ?? 'Daily Summary',
    schedule: overrides.schedule ?? 'daily 09:00',
    prompt: overrides.prompt ?? 'Summarize the day',
    enabled: overrides.enabled ?? true,
    run_if_missed: overrides.run_if_missed ?? false,
    last_run: overrides.last_run ?? null,
  };
}

function makeResult(overrides: Partial<{
  task_name: string;
  timestamp: string;
  success: boolean;
  response: string;
  task_id: string;
}> = {}) {
  return {
    task_id: overrides.task_id ?? 'task-1',
    task_name: overrides.task_name ?? 'Daily Summary',
    timestamp: overrides.timestamp ?? '2024-01-01T09:00:00Z',
    success: overrides.success ?? true,
    response: overrides.response ?? 'Task completed successfully.',
  };
}

function setupSettings(overrides: {
  config?: Record<string, any>;
  tasks?: ReturnType<typeof makeTask>[];
  schedulerEnabled?: boolean;
  results?: ReturnType<typeof makeResult>[];
  settings?: Record<string, any>;
} = {}) {
  const config = makeConfig(overrides.config ?? {});
  const setConfig = vi.fn();
  const setSchedulerEnabled = vi.fn();
  const loadSchedulerData = vi.fn();

  vi.mocked(useSettings).mockReturnValue({
    config,
    setConfig,
    scheduledTasks: overrides.tasks ?? [],
    schedulerEnabled: overrides.schedulerEnabled ?? false,
    setSchedulerEnabled,
    schedulerResults: overrides.results ?? [],
    loadSchedulerData,
    ...(overrides.settings ?? {}),
  } as ReturnType<typeof useSettings>);

  return { config, setConfig, setSchedulerEnabled, loadSchedulerData };
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
      scheduledTasks: [],
      schedulerEnabled: false,
      setSchedulerEnabled: vi.fn(),
      schedulerResults: [],
      loadSchedulerData: vi.fn(),
    } as ReturnType<typeof useSettings>);

    const { container } = render(<AutomationTab />);
    expect(container.firstChild).toBeNull();
  });

  it('renders Scheduler and Tasks headings', () => {
    setupSettings();
    render(<AutomationTab />);
    expect(screen.getByText('Scheduler')).toBeInTheDocument();
    expect(screen.getByText('Tasks')).toBeInTheDocument();
  });
});

// ===================================================================
// 2. Scheduler toggle
// ===================================================================
describe('Scheduler toggle', () => {
  it('calls set_scheduler_enabled with true when toggled on', async () => {
    const user = userEvent.setup();
    const { setSchedulerEnabled } = setupSettings({ schedulerEnabled: false });

    render(<AutomationTab />);
    const checkbox = screen.getByRole('checkbox', { name: /Enable Scheduler/i });
    await user.click(checkbox);

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('set_scheduler_enabled', { enabled: true });
    });
    expect(setSchedulerEnabled).toHaveBeenCalledWith(true);
  });

  it('calls set_scheduler_enabled with false when toggled off', async () => {
    const user = userEvent.setup();
    const { setSchedulerEnabled } = setupSettings({ schedulerEnabled: true });

    render(<AutomationTab />);
    const checkbox = screen.getByRole('checkbox', { name: /Enable Scheduler/i });
    await user.click(checkbox);

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('set_scheduler_enabled', { enabled: false });
    });
    expect(setSchedulerEnabled).toHaveBeenCalledWith(false);
  });
});

// ===================================================================
// 3. Tasks empty state
// ===================================================================
describe('Tasks empty state', () => {
  it('shows "No scheduled tasks. Add one to get started." when tasks list is empty', () => {
    setupSettings({ tasks: [] });
    render(<AutomationTab />);
    expect(screen.getByText(/No scheduled tasks\. Add one to get started\./i)).toBeInTheDocument();
  });
});

// ===================================================================
// 4. Add task form
// ===================================================================
describe('Add task form', () => {
  it('shows the add form when "+ Add Scheduled Task" button is clicked', async () => {
    const user = userEvent.setup();
    setupSettings();
    render(<AutomationTab />);

    await user.click(screen.getByRole('button', { name: '+ Add Scheduled Task' }));

    expect(screen.getByPlaceholderText(/Task name/i)).toBeInTheDocument();
    expect(screen.getByPlaceholderText(/Task prompt/i)).toBeInTheDocument();
  });

  it('add task form has name, schedule, and prompt fields', async () => {
    const user = userEvent.setup();
    setupSettings();
    render(<AutomationTab />);

    await user.click(screen.getByRole('button', { name: '+ Add Scheduled Task' }));

    expect(screen.getByPlaceholderText(/Task name/i)).toBeInTheDocument();
    // Schedule select
    expect(screen.getByDisplayValue('Daily at 9:00 AM')).toBeInTheDocument();
    // Prompt textarea
    expect(screen.getByPlaceholderText(/Task prompt/i)).toBeInTheDocument();
  });

  it('Add Task button is disabled when name and prompt are empty', async () => {
    const user = userEvent.setup();
    setupSettings();
    render(<AutomationTab />);

    await user.click(screen.getByRole('button', { name: '+ Add Scheduled Task' }));

    expect(screen.getByRole('button', { name: 'Add Task' })).toBeDisabled();
  });

  it('Add Task calls invoke add_scheduled_task with correct args', async () => {
    const user = userEvent.setup();
    const { loadSchedulerData } = setupSettings();
    render(<AutomationTab />);

    await user.click(screen.getByRole('button', { name: '+ Add Scheduled Task' }));

    const nameInput = screen.getByPlaceholderText(/Task name/i);
    const promptInput = screen.getByPlaceholderText(/Task prompt/i);

    fireEvent.change(nameInput, { target: { value: 'Morning Summary' } });
    fireEvent.change(promptInput, { target: { value: 'Summarize my day' } });

    await user.click(screen.getByRole('button', { name: 'Add Task' }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('add_scheduled_task', {
        name: 'Morning Summary',
        schedule: 'daily 09:00',
        prompt: 'Summarize my day',
      });
    });
    expect(loadSchedulerData).toHaveBeenCalled();
  });

  it('Cancel button hides the add form', async () => {
    const user = userEvent.setup();
    setupSettings();
    render(<AutomationTab />);

    await user.click(screen.getByRole('button', { name: '+ Add Scheduled Task' }));
    expect(screen.getByPlaceholderText(/Task name/i)).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'Cancel' }));

    expect(screen.queryByPlaceholderText(/Task name/i)).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: '+ Add Scheduled Task' })).toBeInTheDocument();
  });
});

// ===================================================================
// 5. Existing tasks
// ===================================================================
describe('Existing tasks', () => {
  it('shows task name and schedule', () => {
    setupSettings({
      tasks: [makeTask({ name: 'Daily Summary', schedule: 'daily 09:00' })],
    });
    render(<AutomationTab />);

    expect(screen.getByText('Daily Summary')).toBeInTheDocument();
    expect(screen.getByText('daily 09:00')).toBeInTheDocument();
  });

  it('Enable/Disable button calls update_scheduled_task with toggled state', async () => {
    const user = userEvent.setup();
    const { loadSchedulerData } = setupSettings({
      tasks: [makeTask({ id: 'task-1', enabled: true })],
    });

    render(<AutomationTab />);
    await user.click(screen.getByRole('button', { name: 'Disable' }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('update_scheduled_task', {
        taskId: 'task-1',
        enabled: false,
      });
    });
    expect(loadSchedulerData).toHaveBeenCalled();
  });

  it('Run Now button calls trigger_scheduled_task', async () => {
    const user = userEvent.setup();
    const { loadSchedulerData } = setupSettings({
      tasks: [makeTask({ id: 'task-1' })],
    });

    render(<AutomationTab />);
    await user.click(screen.getByRole('button', { name: 'Run Now' }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('trigger_scheduled_task', { taskId: 'task-1' });
    });
    expect(loadSchedulerData).toHaveBeenCalled();
  });

  it('Remove button calls remove_scheduled_task after confirmation', async () => {
    const user = userEvent.setup();
    mockConfirm.mockResolvedValue(true);
    const { loadSchedulerData } = setupSettings({
      tasks: [makeTask({ id: 'task-1' })],
    });

    render(<AutomationTab />);
    await user.click(screen.getByRole('button', { name: 'Remove' }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('remove_scheduled_task', { taskId: 'task-1' });
    });
    expect(loadSchedulerData).toHaveBeenCalled();
  });

  it('Remove button does NOT call remove_scheduled_task when confirm is cancelled', async () => {
    const user = userEvent.setup();
    mockConfirm.mockResolvedValue(false);
    setupSettings({ tasks: [makeTask({ id: 'task-1' })] });

    render(<AutomationTab />);
    await user.click(screen.getByRole('button', { name: 'Remove' }));

    expect(vi.mocked(invoke)).not.toHaveBeenCalledWith('remove_scheduled_task', expect.anything());
  });
});

// ===================================================================
// 6. Scheduler results
// ===================================================================
describe('Scheduler results', () => {
  it('results section is hidden when schedulerResults is empty', () => {
    setupSettings({ results: [] });
    render(<AutomationTab />);
    expect(screen.queryByText(/Recent Results/i)).not.toBeInTheDocument();
  });

  it('shows results section when schedulerResults has items and heading is clicked', async () => {
    const user = userEvent.setup();
    setupSettings({
      results: [makeResult({ task_name: 'Daily Summary', response: 'Done well.' })],
    });

    render(<AutomationTab />);

    // Results heading visible (collapsed)
    const heading = screen.getByText(/Recent Results/i);
    expect(heading).toBeInTheDocument();

    // Click to expand
    await user.click(heading);

    expect(screen.getByText('Daily Summary')).toBeInTheDocument();
    expect(screen.getByText('Done well.')).toBeInTheDocument();
  });
});

// ===================================================================
// 7. Webhooks section
// ===================================================================
describe('Webhooks section', () => {
  it('Enable Webhook calls set_webhook_enabled and setConfig', async () => {
    const user = userEvent.setup();
    const { setConfig, config } = setupSettings({
      config: { webhooks: { enabled: false, port: 18791, auth_token: undefined, endpoints: [], tls: { enabled: false, auto_generate: true }, rate_limit: { max_attempts: 10, window_secs: 60, lockout_secs: 300 } } },
    });

    render(<AutomationTab />);
    await user.click(screen.getByRole('checkbox', { name: /Enable Webhook Server/i }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('set_webhook_enabled', { enabled: true });
    });
    expect(setConfig).toHaveBeenCalledWith(expect.objectContaining({
      webhooks: expect.objectContaining({ enabled: true }),
    }));
  });

  it('port hint is shown when webhook is enabled', () => {
    setupSettings({
      config: {
        webhooks: {
          enabled: true,
          port: 18791,
          auth_token: 'mytoken',
          endpoints: [],
          tls: { enabled: false, auto_generate: true },
          rate_limit: { max_attempts: 10, window_secs: 60, lockout_secs: 300 },
        },
      },
    });

    render(<AutomationTab />);
    expect(screen.getByText(/Port: 18791/i)).toBeInTheDocument();
  });

  it('Bearer Token field is shown when webhook is enabled', () => {
    setupSettings({
      config: {
        webhooks: {
          enabled: true,
          port: 18791,
          auth_token: 'abc-token',
          endpoints: [],
          tls: { enabled: false, auto_generate: true },
          rate_limit: { max_attempts: 10, window_secs: 60, lockout_secs: 300 },
        },
      },
    });

    render(<AutomationTab />);
    expect(screen.getByText('Bearer Token')).toBeInTheDocument();
    expect(screen.getByDisplayValue('abc-token')).toBeInTheDocument();
  });

  it('Regenerate Token button calls regenerate_webhook_token and updates config', async () => {
    const user = userEvent.setup();
    const { setConfig } = setupSettings({
      config: {
        webhooks: {
          enabled: true,
          port: 18791,
          auth_token: 'old-token',
          endpoints: [],
          tls: { enabled: false, auto_generate: true },
          rate_limit: { max_attempts: 10, window_secs: 60, lockout_secs: 300 },
        },
      },
    });
    vi.mocked(invoke).mockResolvedValue('new-token');

    render(<AutomationTab />);
    await user.click(screen.getByRole('button', { name: 'Regenerate' }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('regenerate_webhook_token');
    });
    expect(setConfig).toHaveBeenCalledWith(expect.objectContaining({
      webhooks: expect.objectContaining({ auth_token: 'new-token' }),
    }));
  });
});

// ===================================================================
// 8. Webhook endpoints section
// ===================================================================
describe('Webhook endpoints section', () => {
  it('shows "No webhook endpoints configured." when endpoints array is empty', () => {
    setupSettings({
      config: {
        webhooks: {
          enabled: true,
          port: 18791,
          auth_token: 'token',
          endpoints: [],
          tls: { enabled: false, auto_generate: true },
          rate_limit: { max_attempts: 10, window_secs: 60, lockout_secs: 300 },
        },
      },
    });

    render(<AutomationTab />);
    expect(screen.getByText(/No webhook endpoints configured\./i)).toBeInTheDocument();
  });

  it('shows existing endpoint name and action label', () => {
    setupSettings({
      config: {
        webhooks: {
          enabled: true,
          port: 18791,
          auth_token: 'token',
          endpoints: [{ id: 'ep-1', name: 'My Endpoint', action: 'TriggerTask', target: 'task-1' }],
          tls: { enabled: false, auto_generate: true },
          rate_limit: { max_attempts: 10, window_secs: 60, lockout_secs: 300 },
        },
      },
    });

    render(<AutomationTab />);
    expect(screen.getByText('My Endpoint')).toBeInTheDocument();
    expect(screen.getByText('Trigger Task')).toBeInTheDocument();
  });

  it('Remove endpoint calls remove_webhook_endpoint and updates config', async () => {
    const user = userEvent.setup();
    const { setConfig } = setupSettings({
      config: {
        webhooks: {
          enabled: true,
          port: 18791,
          auth_token: 'token',
          endpoints: [{ id: 'ep-1', name: 'My Endpoint', action: 'TriggerTask', target: 'task-1' }],
          tls: { enabled: false, auto_generate: true },
          rate_limit: { max_attempts: 10, window_secs: 60, lockout_secs: 300 },
        },
      },
    });

    render(<AutomationTab />);
    await user.click(screen.getByRole('button', { name: 'Remove' }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('remove_webhook_endpoint', { endpointId: 'ep-1' });
    });
    expect(setConfig).toHaveBeenCalled();
  });

  it('shows + Add Webhook Endpoint button and opens add endpoint form', async () => {
    const user = userEvent.setup();
    setupSettings({
      config: {
        webhooks: {
          enabled: true,
          port: 18791,
          auth_token: 'token',
          endpoints: [],
          tls: { enabled: false, auto_generate: true },
          rate_limit: { max_attempts: 10, window_secs: 60, lockout_secs: 300 },
        },
      },
    });

    render(<AutomationTab />);
    await user.click(screen.getByRole('button', { name: '+ Add Webhook Endpoint' }));

    expect(screen.getByPlaceholderText('Endpoint name')).toBeInTheDocument();
  });
});

// ===================================================================
// 9. Webhook TLS section
// ===================================================================
describe('Webhook TLS section', () => {
  it('shows TLS checkbox when webhooks are enabled', () => {
    setupSettings({
      config: {
        webhooks: {
          enabled: true,
          port: 18791,
          auth_token: 'token',
          endpoints: [],
          tls: { enabled: false, auto_generate: true },
          rate_limit: { max_attempts: 10, window_secs: 60, lockout_secs: 300 },
        },
      },
    });

    render(<AutomationTab />);
    expect(screen.getByRole('checkbox', { name: /Enable TLS/i })).toBeInTheDocument();
  });

  it('calls setConfig when TLS checkbox is toggled', async () => {
    const user = userEvent.setup();
    const { setConfig } = setupSettings({
      config: {
        webhooks: {
          enabled: true,
          port: 18791,
          auth_token: 'token',
          endpoints: [],
          tls: { enabled: false, auto_generate: true },
          rate_limit: { max_attempts: 10, window_secs: 60, lockout_secs: 300 },
        },
      },
    });

    render(<AutomationTab />);
    await user.click(screen.getByRole('checkbox', { name: /Enable TLS/i }));

    expect(setConfig).toHaveBeenCalledWith(expect.objectContaining({
      webhooks: expect.objectContaining({
        tls: expect.objectContaining({ enabled: true }),
      }),
    }));
  });
});
