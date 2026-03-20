import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { SecurityTab } from './SecurityTab';
import { invoke } from '@tauri-apps/api/core';
import { useSettings } from '../SettingsContext';

// -------------------------------------------------------------------
// Mocks
// -------------------------------------------------------------------
vi.mock('../SettingsContext');
vi.mock('../../AutonomousModePanel', () => ({
  default: () => <div data-testid="autonomous-mode-panel" />,
}));
vi.mock('../shared/InfoTip', () => ({ InfoTip: () => null }));
vi.mock('../shared/TagInput', () => ({
  TagInput: ({
    tags,
    placeholder,
  }: {
    tags: string[];
    onChange: (tags: string[]) => void;
    placeholder?: string;
  }) => (
    <div>
      {(tags ?? []).map((t) => (
        <span key={t} className="tag">
          {t}
        </span>
      ))}
      <input placeholder={placeholder} />
    </div>
  ),
}));
vi.mock('../shared/suggestions', () => ({
  PATTERN_SUGGESTIONS: [],
  SAFE_COMMAND_SUGGESTIONS: [],
  DANGEROUS_COMMAND_SUGGESTIONS: [],
  SENSITIVE_PATH_SUGGESTIONS: [],
  SAFE_PATH_SUGGESTIONS: [],
  ALLOWED_DOMAIN_SUGGESTIONS: [],
  BLOCKED_DOMAIN_SUGGESTIONS: [],
}));
// invoke is mocked globally via setup.ts

// -------------------------------------------------------------------
// Config fixture
// -------------------------------------------------------------------
const defaultGuardrails = () => ({
  security_level: 'Standard',
  block_destructive_commands: true,
  block_sensitive_data_sharing: true,
  detect_prompt_injection: true,
  block_prompt_injection: false,
  confirm_external_actions: false,
  use_dcg: true,
  dangers_acknowledged: false,
  default_tool_permission: 'AllowWithLogging',
  dangerous_tool_patterns: [] as string[],
});

const defaultDefense = () => ({
  enabled: false,
  fail_open: true,
  deberta_enabled: true,
  deberta_threshold: 0.5,
  deberta_model_path: undefined as string | undefined,
  llama_guard_enabled: false,
  llama_guard_mode: 'api',
  llama_guard_api_url: 'http://localhost:11434',
  allow_remote_llama_guard: false,
});

const defaultExecute = () => ({
  enabled: false,
  use_dcg: true,
  skill_runtime_exec_enabled: false,
  default_timeout_ms: 30000,
  max_output_bytes: 1048576,
  working_directory: undefined as string | undefined,
  allowed_commands: [] as string[],
  blocked_commands: [] as string[],
});

const defaultFilesystem = () => ({
  allowed_paths: [] as string[],
  blocked_paths: [] as string[],
  max_read_bytes: 1048576,
  max_write_bytes: 10485760,
});

const defaultFetch = () => ({
  allowed_domains: [] as string[],
  blocked_domains: [] as string[],
  max_response_bytes: 1048576,
  default_timeout_ms: 30000,
});

const defaultWebhooks = () => ({
  enabled: false,
  port: 18791,
  auth_token: undefined as string | undefined,
  tls: {
    enabled: false,
    auto_generate: false,
    cert_path: undefined as string | undefined,
    key_path: undefined as string | undefined,
  },
  rate_limit: { max_attempts: 10, window_secs: 60, lockout_secs: 300 },
});

const makeConfig = (
  overrides: {
    guardrails?: Partial<ReturnType<typeof defaultGuardrails>>;
    defense?: Partial<ReturnType<typeof defaultDefense>>;
    execute?: Partial<ReturnType<typeof defaultExecute>>;
    filesystem?: Partial<ReturnType<typeof defaultFilesystem>>;
    fetch?: Partial<ReturnType<typeof defaultFetch>>;
    webhooks?: Partial<ReturnType<typeof defaultWebhooks>>;
  } = {},
) => ({
  guardrails: { ...defaultGuardrails(), ...(overrides.guardrails ?? {}) },
  defense: { ...defaultDefense(), ...(overrides.defense ?? {}) },
  execute: { ...defaultExecute(), ...(overrides.execute ?? {}) },
  filesystem: { ...defaultFilesystem(), ...(overrides.filesystem ?? {}) },
  fetch: { ...defaultFetch(), ...(overrides.fetch ?? {}) },
  webhooks: { ...defaultWebhooks(), ...(overrides.webhooks ?? {}) },
});

type Config = ReturnType<typeof makeConfig>;

function setupSettings(
  config: Config | null = makeConfig(),
  extra: {
    defenseStatus?: {
      deberta_healthy: boolean;
      deberta_loaded: boolean;
      llama_guard_available: boolean;
    } | null;
    toolPermissions?: Record<
      string,
      { default_permission: string; tool_overrides: Record<string, string> }
    >;
    mcpServers?: Array<{ name: string }>;
  } = {},
) {
  const setConfig = vi.fn();
  const loadToolPermissions = vi.fn();

  vi.mocked(useSettings).mockReturnValue({
    config,
    setConfig,
    defenseStatus: extra.defenseStatus ?? null,
    toolPermissions: extra.toolPermissions ?? {},
    loadToolPermissions,
    mcpServers: extra.mcpServers ?? [],
  } as ReturnType<typeof useSettings>);

  return { setConfig, loadToolPermissions };
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
  it('renders null when config is null', () => {
    setupSettings(null);
    const { container } = render(<SecurityTab />);
    expect(container.firstChild).toBeNull();
  });

  it('renders tab-content div when config is present', () => {
    setupSettings(makeConfig());
    const { container } = render(<SecurityTab />);
    expect(container.querySelector('.tab-content')).toBeInTheDocument();
  });

  it('renders AutonomousModePanel', () => {
    setupSettings(makeConfig());
    render(<SecurityTab />);
    expect(screen.getByTestId('autonomous-mode-panel')).toBeInTheDocument();
  });
});

// ===================================================================
// 2. Security level select
// ===================================================================
describe('Security level select', () => {
  it('shows the current security level in the select', () => {
    setupSettings(makeConfig({ guardrails: { security_level: 'Maximum' } }));
    render(<SecurityTab />);
    const select = screen.getByDisplayValue(/Maximum/i) as HTMLSelectElement;
    expect(select.value).toBe('Maximum');
  });

  it('shows warning banner when security_level is Relaxed', () => {
    setupSettings(makeConfig({ guardrails: { security_level: 'Relaxed' } }));
    render(<SecurityTab />);
    expect(screen.getByText(/Security is relaxed/i)).toBeInTheDocument();
  });

  it('shows warning banner when security_level is Disabled', () => {
    setupSettings(makeConfig({ guardrails: { security_level: 'Disabled' } }));
    render(<SecurityTab />);
    expect(screen.getByText(/All security protections are disabled/i)).toBeInTheDocument();
  });

  it('does not show warning banner when security_level is Standard', () => {
    setupSettings(makeConfig({ guardrails: { security_level: 'Standard' } }));
    render(<SecurityTab />);
    expect(screen.queryByText(/Security is relaxed/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/All security protections are disabled/i)).not.toBeInTheDocument();
  });
});

// ===================================================================
// 3. Guardrails checkboxes
// ===================================================================
describe('Guardrails checkboxes', () => {
  it('toggling block_destructive_commands calls setConfig with updated value', async () => {
    const user = userEvent.setup();
    const config = makeConfig();
    const { setConfig } = setupSettings(config);
    render(<SecurityTab />);

    const checkbox = screen.getByRole('checkbox', {
      name: /Block destructive commands/i,
    });
    await user.click(checkbox);

    expect(setConfig).toHaveBeenCalledWith(
      expect.objectContaining({
        guardrails: expect.objectContaining({ block_destructive_commands: false }),
      }),
    );
  });

  it('toggling block_sensitive_data_sharing calls setConfig', async () => {
    const user = userEvent.setup();
    const config = makeConfig();
    const { setConfig } = setupSettings(config);
    render(<SecurityTab />);

    const checkbox = screen.getByRole('checkbox', { name: /Block sensitive data sharing/i });
    await user.click(checkbox);

    expect(setConfig).toHaveBeenCalledWith(
      expect.objectContaining({
        guardrails: expect.objectContaining({ block_sensitive_data_sharing: false }),
      }),
    );
  });

  it('toggling detect_prompt_injection calls setConfig', async () => {
    const user = userEvent.setup();
    const config = makeConfig();
    const { setConfig } = setupSettings(config);
    render(<SecurityTab />);

    const checkbox = screen.getByRole('checkbox', { name: /Detect prompt injection/i });
    await user.click(checkbox);

    expect(setConfig).toHaveBeenCalledWith(
      expect.objectContaining({
        guardrails: expect.objectContaining({ detect_prompt_injection: false }),
      }),
    );
  });

  it('toggling block_prompt_injection calls setConfig', async () => {
    const user = userEvent.setup();
    const config = makeConfig();
    const { setConfig } = setupSettings(config);
    render(<SecurityTab />);

    const checkbox = screen.getByRole('checkbox', {
      name: /Block prompt injection \(not just warn\)/i,
    });
    await user.click(checkbox);

    expect(setConfig).toHaveBeenCalledWith(
      expect.objectContaining({
        guardrails: expect.objectContaining({ block_prompt_injection: true }),
      }),
    );
  });

  it('toggling confirm_external_actions calls setConfig', async () => {
    const user = userEvent.setup();
    const config = makeConfig();
    const { setConfig } = setupSettings(config);
    render(<SecurityTab />);

    const checkbox = screen.getByRole('checkbox', {
      name: /Require confirmation for external actions/i,
    });
    await user.click(checkbox);

    expect(setConfig).toHaveBeenCalledWith(
      expect.objectContaining({
        guardrails: expect.objectContaining({ confirm_external_actions: true }),
      }),
    );
  });
});

// ===================================================================
// 4. dangers_acknowledged warning
// ===================================================================
describe('dangers_acknowledged warning', () => {
  it('shows warning when dangers_acknowledged is true', () => {
    setupSettings(makeConfig({ guardrails: { dangers_acknowledged: true } }));
    render(<SecurityTab />);
    expect(
      screen.getByText(/You have acknowledged reduced security/i),
    ).toBeInTheDocument();
  });

  it('does not show warning when dangers_acknowledged is false', () => {
    setupSettings(makeConfig({ guardrails: { dangers_acknowledged: false } }));
    render(<SecurityTab />);
    expect(
      screen.queryByText(/You have acknowledged reduced security/i),
    ).not.toBeInTheDocument();
  });
});

// ===================================================================
// 5. Default tool permission select
// ===================================================================
describe('Default tool permission select', () => {
  it('shows the current default_tool_permission value', () => {
    setupSettings(makeConfig({ guardrails: { default_tool_permission: 'RequireConfirmation' } }));
    render(<SecurityTab />);
    const select = screen.getByDisplayValue(/Require Confirmation/i) as HTMLSelectElement;
    expect(select.value).toBe('RequireConfirmation');
  });

  it('changing default_tool_permission calls setConfig', () => {
    const config = makeConfig();
    const { setConfig } = setupSettings(config);
    render(<SecurityTab />);

    // The default is AllowWithLogging; there's a select for default_tool_permission
    // and possibly one in tool permissions section, but tool permissions section
    // shows nothing by default (empty). Get the guardrails one by label context.
    const allSelects = screen.getAllByRole('combobox');
    // security_level is first, default_tool_permission is second
    const permSelect = allSelects[1];
    fireEvent.change(permSelect, { target: { value: 'Block' } });

    expect(setConfig).toHaveBeenCalledWith(
      expect.objectContaining({
        guardrails: expect.objectContaining({ default_tool_permission: 'Block' }),
      }),
    );
  });
});

// ===================================================================
// 6. Defense pipeline toggle
// ===================================================================
describe('Defense pipeline toggle', () => {
  it('hides advanced defense options when defense.enabled is false', () => {
    setupSettings(makeConfig({ defense: { enabled: false } }));
    render(<SecurityTab />);
    // The fail_open checkbox only appears when enabled
    expect(
      screen.queryByRole('checkbox', { name: /Allow messages when models are unavailable/i }),
    ).not.toBeInTheDocument();
  });

  it('shows advanced defense options when defense.enabled is true', () => {
    setupSettings(
      makeConfig({
        defense: {
          enabled: true,
          fail_open: true,
          deberta_enabled: false,
          llama_guard_enabled: false,
        },
      }),
    );
    render(<SecurityTab />);
    expect(
      screen.getByRole('checkbox', { name: /Allow messages when models are unavailable/i }),
    ).toBeInTheDocument();
  });
});

// ===================================================================
// 7. Defense status indicator
// ===================================================================
describe('Defense status indicator', () => {
  it('shows "Pipeline active" when deberta_healthy is true', () => {
    setupSettings(
      makeConfig({
        defense: {
          enabled: true,
          fail_open: true,
          deberta_enabled: false,
          llama_guard_enabled: false,
        },
      }),
      {
        defenseStatus: {
          deberta_healthy: true,
          deberta_loaded: false,
          llama_guard_available: false,
        },
      },
    );
    render(<SecurityTab />);
    expect(screen.getByText(/Pipeline active/i)).toBeInTheDocument();
  });

  it('shows "Pipeline degraded" when deberta_healthy is false', () => {
    setupSettings(
      makeConfig({
        defense: {
          enabled: true,
          fail_open: true,
          deberta_enabled: false,
          llama_guard_enabled: false,
        },
      }),
      {
        defenseStatus: {
          deberta_healthy: false,
          deberta_loaded: false,
          llama_guard_available: false,
        },
      },
    );
    render(<SecurityTab />);
    expect(screen.getByText(/Pipeline degraded/i)).toBeInTheDocument();
  });

  it('shows "DeBERTa loaded" hint when deberta_loaded is true', () => {
    setupSettings(
      makeConfig({
        defense: {
          enabled: true,
          fail_open: true,
          deberta_enabled: false,
          llama_guard_enabled: false,
        },
      }),
      {
        defenseStatus: {
          deberta_healthy: true,
          deberta_loaded: true,
          llama_guard_available: false,
        },
      },
    );
    render(<SecurityTab />);
    expect(screen.getByText(/DeBERTa loaded/i)).toBeInTheDocument();
  });
});

// ===================================================================
// 8. fail_open warning
// ===================================================================
describe('fail_open warning', () => {
  it('shows warning when fail_open is false', () => {
    setupSettings(
      makeConfig({
        defense: {
          enabled: true,
          fail_open: false,
          deberta_enabled: false,
          llama_guard_enabled: false,
        },
      }),
    );
    render(<SecurityTab />);
    expect(
      screen.getByText(/All messages will be BLOCKED if defense models fail/i),
    ).toBeInTheDocument();
  });

  it('does not show warning when fail_open is true', () => {
    setupSettings(
      makeConfig({
        defense: {
          enabled: true,
          fail_open: true,
          deberta_enabled: false,
          llama_guard_enabled: false,
        },
      }),
    );
    render(<SecurityTab />);
    expect(
      screen.queryByText(/All messages will be BLOCKED if defense models fail/i),
    ).not.toBeInTheDocument();
  });
});

// ===================================================================
// 9. DeBERTa sub-section
// ===================================================================
describe('DeBERTa sub-section', () => {
  it('hides threshold slider when deberta_enabled is false', () => {
    setupSettings(
      makeConfig({
        defense: {
          enabled: true,
          fail_open: true,
          deberta_enabled: false,
          llama_guard_enabled: false,
        },
      }),
    );
    render(<SecurityTab />);
    expect(screen.queryByRole('slider')).not.toBeInTheDocument();
  });

  it('shows threshold slider when deberta_enabled is true', () => {
    setupSettings(
      makeConfig({
        defense: {
          enabled: true,
          fail_open: true,
          deberta_enabled: true,
          deberta_threshold: 0.5,
          llama_guard_enabled: false,
        },
      }),
    );
    render(<SecurityTab />);
    expect(screen.getByRole('slider')).toBeInTheDocument();
  });
});

// ===================================================================
// 10. Llama Guard sub-section
// ===================================================================
describe('Llama Guard sub-section', () => {
  it('hides llama guard options when llama_guard_enabled is false', () => {
    setupSettings(
      makeConfig({
        defense: {
          enabled: true,
          fail_open: true,
          deberta_enabled: false,
          llama_guard_enabled: false,
        },
      }),
    );
    render(<SecurityTab />);
    expect(
      screen.queryByPlaceholderText('http://localhost:11434'),
    ).not.toBeInTheDocument();
  });

  it('shows llama guard options when llama_guard_enabled is true', () => {
    setupSettings(
      makeConfig({
        defense: {
          enabled: true,
          fail_open: true,
          deberta_enabled: false,
          llama_guard_enabled: true,
          llama_guard_mode: 'api',
          llama_guard_api_url: 'http://localhost:11434',
          allow_remote_llama_guard: false,
        },
      }),
    );
    render(<SecurityTab />);
    expect(screen.getByPlaceholderText('http://localhost:11434')).toBeInTheDocument();
  });

  it('shows allow_remote warning when allow_remote_llama_guard is true', () => {
    setupSettings(
      makeConfig({
        defense: {
          enabled: true,
          fail_open: true,
          deberta_enabled: false,
          llama_guard_enabled: true,
          llama_guard_mode: 'api',
          llama_guard_api_url: 'http://localhost:11434',
          allow_remote_llama_guard: true,
        },
      }),
    );
    render(<SecurityTab />);
    expect(
      screen.getByText(/Remote endpoints send conversation data over the network/i),
    ).toBeInTheDocument();
  });
});

// ===================================================================
// 11. Security audit
// ===================================================================
describe('Security audit', () => {
  it('renders the "Run Security Audit" button', () => {
    setupSettings(makeConfig());
    render(<SecurityTab />);
    expect(
      screen.getByRole('button', { name: /Run Security Audit/i }),
    ).toBeInTheDocument();
  });

  it('button is disabled while audit is running', async () => {
    const user = userEvent.setup();
    // Never resolves so runningAudit stays true
    vi.mocked(invoke).mockImplementation(() => new Promise(() => {}));
    setupSettings(makeConfig());
    render(<SecurityTab />);

    const button = screen.getByRole('button', { name: /Run Security Audit/i });
    await user.click(button);

    await waitFor(() =>
      expect(screen.getByRole('button', { name: /Running Audit.../i })).toBeDisabled(),
    );
  });

  it('calls invoke("run_security_audit") when button is clicked', async () => {
    const user = userEvent.setup();
    const report = { findings: [], passed_count: 5, total_checks: 5 };
    vi.mocked(invoke).mockResolvedValue(JSON.stringify(report));
    setupSettings(makeConfig());
    render(<SecurityTab />);

    await user.click(screen.getByRole('button', { name: /Run Security Audit/i }));

    await waitFor(() =>
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('run_security_audit'),
    );
  });

  it('shows pass count after audit completes', async () => {
    const user = userEvent.setup();
    const report = { findings: [], passed_count: 7, total_checks: 10 };
    vi.mocked(invoke).mockResolvedValue(JSON.stringify(report));
    setupSettings(makeConfig());
    render(<SecurityTab />);

    await user.click(screen.getByRole('button', { name: /Run Security Audit/i }));

    await waitFor(() => expect(screen.getByText(/7\/10/)).toBeInTheDocument());
  });

  it('shows auto-fixable count when findings exist', async () => {
    const user = userEvent.setup();
    const report = {
      findings: [
        {
          id: 'f1',
          severity: 'High',
          title: 'Issue A',
          description: 'Desc A',
          fix_hint: null,
          auto_fixable: true,
        },
        {
          id: 'f2',
          severity: 'Medium',
          title: 'Issue B',
          description: 'Desc B',
          fix_hint: null,
          auto_fixable: false,
        },
      ],
      passed_count: 8,
      total_checks: 10,
    };
    vi.mocked(invoke).mockResolvedValue(JSON.stringify(report));
    setupSettings(makeConfig());
    render(<SecurityTab />);

    await user.click(screen.getByRole('button', { name: /Run Security Audit/i }));

    await waitFor(() => screen.getByText(/8\/10/));
    // The auto-fixable count is rendered as: "— <strong>1</strong> auto-fixable"
    // Use getAllByText to check there's a <strong> with "1" near "auto-fixable"
    const infoText = screen.getByText(/8\/10/).closest('.info-text') as HTMLElement;
    expect(within(infoText).getByText('1')).toBeInTheDocument();
    expect(infoText.textContent).toMatch(/auto-fixable/i);
  });

  it('shows finding severity and title in mcp-server-card', async () => {
    const user = userEvent.setup();
    const report = {
      findings: [
        {
          id: 'f1',
          severity: 'Critical',
          title: 'Critical Finding',
          description: 'Something critical is wrong',
          fix_hint: 'Fix it now',
          auto_fixable: false,
        },
      ],
      passed_count: 0,
      total_checks: 1,
    };
    vi.mocked(invoke).mockResolvedValue(JSON.stringify(report));
    setupSettings(makeConfig());
    render(<SecurityTab />);

    await user.click(screen.getByRole('button', { name: /Run Security Audit/i }));

    await waitFor(() => screen.getByText('Critical Finding'));
    expect(screen.getByText('Critical')).toBeInTheDocument();
    expect(screen.getByText('Something critical is wrong')).toBeInTheDocument();
    expect(screen.getByText(/Fix it now/i)).toBeInTheDocument();
  });

  it('shows "All checks passed" when there are no findings', async () => {
    const user = userEvent.setup();
    const report = { findings: [], passed_count: 17, total_checks: 17 };
    vi.mocked(invoke).mockResolvedValue(JSON.stringify(report));
    setupSettings(makeConfig());
    render(<SecurityTab />);

    await user.click(screen.getByRole('button', { name: /Run Security Audit/i }));

    await waitFor(() =>
      expect(screen.getByText(/All checks passed/i)).toBeInTheDocument(),
    );
  });
});

// ===================================================================
// 12. Auto-Fix
// ===================================================================
describe('Auto-Fix', () => {
  async function renderWithFinding() {
    const user = userEvent.setup();
    const finding = {
      id: 'finding-123',
      severity: 'High',
      title: 'Auto-fixable issue',
      description: 'An issue that can be fixed',
      fix_hint: null,
      auto_fixable: true,
    };
    const report = {
      findings: [finding],
      passed_count: 0,
      total_checks: 1,
    };
    vi.mocked(invoke)
      .mockResolvedValueOnce(JSON.stringify(report)) // run_security_audit
      .mockResolvedValueOnce(undefined) // auto_fix_finding
      .mockResolvedValueOnce(
        JSON.stringify({ findings: [], passed_count: 1, total_checks: 1 }),
      ); // re-run audit

    setupSettings(makeConfig());
    render(<SecurityTab />);
    await user.click(screen.getByRole('button', { name: /Run Security Audit/i }));
    await waitFor(() => screen.getByRole('button', { name: /Auto-Fix/i }));
    return { user };
  }

  it('calls invoke("auto_fix_finding") with correct findingId', async () => {
    const { user } = await renderWithFinding();
    await user.click(screen.getByRole('button', { name: /Auto-Fix/i }));

    await waitFor(() =>
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('auto_fix_finding', {
        findingId: 'finding-123',
      }),
    );
  });

  it('re-runs the audit after auto-fix completes', async () => {
    const { user } = await renderWithFinding();
    await user.click(screen.getByRole('button', { name: /Auto-Fix/i }));

    await waitFor(() =>
      expect(
        vi.mocked(invoke).mock.calls.filter((c) => c[0] === 'run_security_audit'),
      ).toHaveLength(2),
    );
  });
});

// ===================================================================
// 13. Execution security
// ===================================================================
describe('Execution security', () => {
  it('toggling "Enable command execution" calls setConfig', async () => {
    const user = userEvent.setup();
    const config = makeConfig();
    const { setConfig } = setupSettings(config);
    render(<SecurityTab />);

    const checkbox = screen.getByRole('checkbox', { name: /Enable command execution/i });
    await user.click(checkbox);

    expect(setConfig).toHaveBeenCalledWith(
      expect.objectContaining({
        execute: expect.objectContaining({ enabled: true }),
      }),
    );
  });

  it('toggling "Destructive Command Guard" in execute section calls setConfig', async () => {
    const user = userEvent.setup();
    const config = makeConfig();
    const { setConfig } = setupSettings(config);
    render(<SecurityTab />);

    // There are two DCG checkboxes (guardrails.use_dcg and execute.use_dcg)
    const dcgCheckboxes = screen.getAllByRole('checkbox', { name: /Destructive Command Guard/i });
    // The second one belongs to the execute section
    await user.click(dcgCheckboxes[1]);

    expect(setConfig).toHaveBeenCalledWith(
      expect.objectContaining({
        execute: expect.objectContaining({ use_dcg: false }),
      }),
    );
  });

  it('changing default_timeout_ms calls setConfig', () => {
    const config = makeConfig();
    const { setConfig } = setupSettings(config);
    render(<SecurityTab />);

    // Both execute.default_timeout_ms and fetch.default_timeout_ms default to 30000
    const timeoutInputs = screen.getAllByDisplayValue('30000');
    // execute.default_timeout_ms comes first in DOM order
    fireEvent.change(timeoutInputs[0], { target: { value: '60000' } });

    expect(setConfig).toHaveBeenCalledWith(
      expect.objectContaining({
        execute: expect.objectContaining({ default_timeout_ms: 60000 }),
      }),
    );
  });
});

// ===================================================================
// 14. Filesystem security
// ===================================================================
describe('Filesystem security', () => {
  it('changing max_read_bytes calls setConfig with parsed value', () => {
    const config = makeConfig();
    const { setConfig } = setupSettings(config);
    render(<SecurityTab />);

    // execute.max_output_bytes = 1048576, filesystem.max_read_bytes = 1048576,
    // fetch.max_response_bytes = 1048576 — three inputs with that value.
    // execute.max_output_bytes is index 0; filesystem.max_read_bytes is index 1.
    const allMbInputs = screen.getAllByDisplayValue('1048576');
    fireEvent.change(allMbInputs[1], { target: { value: '2097152' } });

    expect(setConfig).toHaveBeenCalledWith(
      expect.objectContaining({
        filesystem: expect.objectContaining({ max_read_bytes: 2097152 }),
      }),
    );
  });

  it('changing max_write_bytes calls setConfig with parsed value', () => {
    const config = makeConfig();
    const { setConfig } = setupSettings(config);
    render(<SecurityTab />);

    const maxWriteInput = screen.getByDisplayValue('10485760');
    fireEvent.change(maxWriteInput, { target: { value: '20971520' } });

    expect(setConfig).toHaveBeenCalledWith(
      expect.objectContaining({
        filesystem: expect.objectContaining({ max_write_bytes: 20971520 }),
      }),
    );
  });
});

// ===================================================================
// 15. Fetch security
// ===================================================================
describe('Fetch security', () => {
  it('changing fetch max_response_bytes calls setConfig', () => {
    const config = makeConfig();
    const { setConfig } = setupSettings(config);
    render(<SecurityTab />);

    // execute.max_output_bytes = index 0, filesystem.max_read_bytes = index 1,
    // fetch.max_response_bytes = index 2
    const allMbInputs = screen.getAllByDisplayValue('1048576');
    fireEvent.change(allMbInputs[2], { target: { value: '4194304' } });

    expect(setConfig).toHaveBeenCalledWith(
      expect.objectContaining({
        fetch: expect.objectContaining({ max_response_bytes: 4194304 }),
      }),
    );
  });
});

// ===================================================================
// 16. Webhook security
// ===================================================================
describe('Webhook security', () => {
  it('toggling "Enable Webhooks" calls setConfig', async () => {
    const user = userEvent.setup();
    const config = makeConfig();
    const { setConfig } = setupSettings(config);
    render(<SecurityTab />);

    const checkbox = screen.getByRole('checkbox', { name: /Enable Webhooks/i });
    await user.click(checkbox);

    expect(setConfig).toHaveBeenCalledWith(
      expect.objectContaining({
        webhooks: expect.objectContaining({ enabled: true }),
      }),
    );
  });

  it('changing port calls setConfig with parsed port number', () => {
    const config = makeConfig();
    const { setConfig } = setupSettings(config);
    render(<SecurityTab />);

    const portInput = screen.getByDisplayValue('18791');
    fireEvent.change(portInput, { target: { value: '9000' } });

    expect(setConfig).toHaveBeenCalledWith(
      expect.objectContaining({
        webhooks: expect.objectContaining({ port: 9000 }),
      }),
    );
  });

  it('changing auth_token calls setConfig', () => {
    const config = makeConfig();
    const { setConfig } = setupSettings(config);
    render(<SecurityTab />);

    const tokenInput = screen.getByPlaceholderText(/Required bearer token/i);
    fireEvent.change(tokenInput, { target: { value: 'mysecrettoken' } });

    expect(setConfig).toHaveBeenCalledWith(
      expect.objectContaining({
        webhooks: expect.objectContaining({ auth_token: 'mysecrettoken' }),
      }),
    );
  });

  it('shows TLS cert/key inputs when tls.enabled is true', () => {
    setupSettings(
      makeConfig({
        webhooks: {
          tls: {
            enabled: true,
            auto_generate: false,
            cert_path: undefined,
            key_path: undefined,
          },
        },
      }),
    );
    render(<SecurityTab />);
    expect(screen.getByPlaceholderText('Path to cert.pem')).toBeInTheDocument();
    expect(screen.getByPlaceholderText('Path to key.pem')).toBeInTheDocument();
  });
});

// ===================================================================
// 17. Tool permissions
// ===================================================================
describe('Tool permissions', () => {
  it('shows empty state message when no MCP servers and no tool permissions', () => {
    setupSettings(makeConfig(), { toolPermissions: {}, mcpServers: [] });
    render(<SecurityTab />);
    expect(
      screen.getByText(/No MCP servers configured/i),
    ).toBeInTheDocument();
  });

  it('shows configured server name in a card', () => {
    setupSettings(makeConfig(), {
      toolPermissions: {
        'my-server': { default_permission: 'AllowWithLogging', tool_overrides: {} },
      },
      mcpServers: [],
    });
    render(<SecurityTab />);
    expect(screen.getByText('my-server')).toBeInTheDocument();
  });

  it('shows override count for configured server', () => {
    setupSettings(makeConfig(), {
      toolPermissions: {
        'my-server': {
          default_permission: 'AllowWithLogging',
          tool_overrides: { tool_a: 'Block', tool_b: 'AutoApprove' },
        },
      },
      mcpServers: [],
    });
    render(<SecurityTab />);
    expect(screen.getByText(/2 overrides/i)).toBeInTheDocument();
  });

  it('changing server default permission calls invoke("update_tool_permissions") and loadToolPermissions', async () => {
    const { loadToolPermissions } = setupSettings(makeConfig(), {
      toolPermissions: {
        'my-server': { default_permission: 'AllowWithLogging', tool_overrides: {} },
      },
      mcpServers: [],
    });
    vi.mocked(invoke).mockResolvedValue(undefined);
    render(<SecurityTab />);

    // The tool-permissions card has its own select; find the one inside the mcp-server-card
    const serverCard = screen
      .getByText('my-server')
      .closest('.mcp-server-card') as HTMLElement;
    const permSelect = within(serverCard).getByRole('combobox');
    fireEvent.change(permSelect, { target: { value: 'Block' } });

    await waitFor(() =>
      expect(vi.mocked(invoke)).toHaveBeenCalledWith(
        'update_tool_permissions',
        expect.objectContaining({
          permissions: expect.objectContaining({
            'my-server': expect.objectContaining({ default_permission: 'Block' }),
          }),
        }),
      ),
    );
    await waitFor(() => expect(loadToolPermissions).toHaveBeenCalled());
  });

  it('shows "Set Custom Permissions" button for unconfigured MCP servers', () => {
    setupSettings(makeConfig(), {
      toolPermissions: {},
      mcpServers: [{ name: 'unconfigured-server' }],
    });
    render(<SecurityTab />);
    expect(screen.getByText('unconfigured-server')).toBeInTheDocument();
    expect(
      screen.getByRole('button', { name: /Set Custom Permissions/i }),
    ).toBeInTheDocument();
  });
});
