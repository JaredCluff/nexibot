import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, within, fireEvent } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ModelsTab } from './ModelsTab';
import { invoke } from '@tauri-apps/api/core';
import { emit } from '@tauri-apps/api/event';
import { useSettings } from '../SettingsContext';

// ─── Mocks ────────────────────────────────────────────────────────────────────

vi.mock('../SettingsContext');
vi.mock('../shared/InfoTip', () => ({ InfoTip: () => null }));
vi.mock('../shared/CollapsibleSection', () => ({
  CollapsibleSection: ({
    title,
    children,
  }: {
    title: string;
    children: React.ReactNode;
  }) => (
    <div
      data-testid={`section-${title.toLowerCase().replace(/[\s()]/g, '-')}`}
    >
      {children}
    </div>
  ),
}));

// ─── Fixtures ─────────────────────────────────────────────────────────────────

const makeConfig = (overrides = {}) => ({
  claude: {
    api_key: undefined,
    model: 'claude-opus-4-6',
    fallback_model: undefined,
    max_tokens: 8192,
    system_prompt: 'You are NexiBot.',
  },
  openai: { api_key: undefined },
  google: undefined,
  deepseek: undefined,
  github_copilot: undefined,
  minimax: undefined,
  ...overrides,
});

const makeModel = (
  id: string,
  display_name: string,
  provider: string,
) => ({ id, display_name, provider, available: true });

function setupSettings({
  config = makeConfig(),
  availableModels = [] as ReturnType<typeof makeModel>[],
  modelsLoading = false,
  loadModels = vi.fn(),
  setConfig = vi.fn(),
} = {}) {
  vi.mocked(useSettings).mockReturnValue({
    config,
    setConfig,
    availableModels,
    modelsLoading,
    loadModels,
  } as ReturnType<typeof useSettings>);
  return { setConfig, loadModels };
}

// ─── Global stubs ─────────────────────────────────────────────────────────────

beforeEach(() => {
  vi.clearAllMocks();
  window.alert = vi.fn();
  window.confirm = vi.fn().mockReturnValue(true);
  vi.mocked(invoke).mockResolvedValue(undefined);
});

// ═══════════════════════════════════════════════════════════════════════════════
// 1. Rendering
// ═══════════════════════════════════════════════════════════════════════════════

describe('Rendering', () => {
  it('returns null when config is null', () => {
    vi.mocked(useSettings).mockReturnValue({
      config: null,
      setConfig: vi.fn(),
      availableModels: [],
      modelsLoading: false,
      loadModels: vi.fn(),
    } as ReturnType<typeof useSettings>);

    const { container } = render(<ModelsTab />);
    expect(container.firstChild).toBeNull();
  });

  it('renders Primary Model heading when config is present', () => {
    setupSettings();
    render(<ModelsTab />);
    expect(screen.getByText('Primary Model')).toBeInTheDocument();
  });
});

// ═══════════════════════════════════════════════════════════════════════════════
// 2. Loading state
// ═══════════════════════════════════════════════════════════════════════════════

describe('Loading state', () => {
  it('shows "Loading available models..." when modelsLoading is true', () => {
    setupSettings({ modelsLoading: true });
    render(<ModelsTab />);
    expect(screen.getByText('Loading available models...')).toBeInTheDocument();
  });
});

// ═══════════════════════════════════════════════════════════════════════════════
// 3. Empty models
// ═══════════════════════════════════════════════════════════════════════════════

describe('Empty models', () => {
  it('shows "No models available" message when availableModels is empty', () => {
    setupSettings({ availableModels: [] });
    render(<ModelsTab />);
    expect(
      screen.getByText(/No models available/i),
    ).toBeInTheDocument();
  });

  it('Retry button calls loadModels when clicked', async () => {
    const user = userEvent.setup();
    const { loadModels } = setupSettings({ availableModels: [] });
    render(<ModelsTab />);

    await user.click(screen.getByRole('button', { name: 'Retry' }));
    expect(loadModels).toHaveBeenCalledTimes(1);
  });
});

// ═══════════════════════════════════════════════════════════════════════════════
// 4. Model selection
// ═══════════════════════════════════════════════════════════════════════════════

describe('Model selection', () => {
  it('renders models grouped by provider', () => {
    setupSettings({
      availableModels: [
        makeModel('claude-opus-4-6', 'Claude Opus 4.6', 'Anthropic'),
        makeModel('claude-sonnet-4', 'Claude Sonnet 4', 'Anthropic'),
        makeModel('gpt-4o', 'GPT-4o', 'OpenAI'),
      ],
    });
    render(<ModelsTab />);

    const modelList = document.querySelector('.model-list')!;

    // Provider headings
    expect(within(modelList as HTMLElement).getByText('Anthropic')).toBeInTheDocument();
    expect(within(modelList as HTMLElement).getByText('OpenAI')).toBeInTheDocument();

    // Model display names within the model list
    expect(within(modelList as HTMLElement).getByText('Claude Opus 4.6')).toBeInTheDocument();
    expect(within(modelList as HTMLElement).getByText('Claude Sonnet 4')).toBeInTheDocument();
    expect(within(modelList as HTMLElement).getByText('GPT-4o')).toBeInTheDocument();
  });

  it('selecting a radio button calls setConfig with the new model id', async () => {
    const user = userEvent.setup();
    const { setConfig } = setupSettings({
      availableModels: [
        makeModel('claude-opus-4-6', 'Claude Opus 4.6', 'Anthropic'),
        makeModel('gpt-4o', 'GPT-4o', 'OpenAI'),
      ],
    });
    render(<ModelsTab />);

    const gptRadio = screen.getByRole('radio', { name: /gpt-4o/i });
    await user.click(gptRadio);

    expect(setConfig).toHaveBeenCalledWith(
      expect.objectContaining({
        claude: expect.objectContaining({ model: 'gpt-4o' }),
      }),
    );
  });

  it('selected model card has "selected" class', () => {
    setupSettings({
      config: makeConfig({
        claude: {
          model: 'claude-opus-4-6',
          max_tokens: 8192,
          system_prompt: '',
          fallback_model: undefined,
          api_key: undefined,
        },
      }),
      availableModels: [
        makeModel('claude-opus-4-6', 'Claude Opus 4.6', 'Anthropic'),
        makeModel('gpt-4o', 'GPT-4o', 'OpenAI'),
      ],
    });
    render(<ModelsTab />);

    // Scope query to the model list to avoid matching fallback select options
    const modelList = document.querySelector('.model-list')!;

    const opusLabel = within(modelList as HTMLElement)
      .getByText('Claude Opus 4.6')
      .closest('label');
    expect(opusLabel).toHaveClass('selected');

    const gptLabel = within(modelList as HTMLElement)
      .getByText('GPT-4o')
      .closest('label');
    expect(gptLabel).not.toHaveClass('selected');
  });

  it('sets max_tokens to 16384 for claude-opus model', async () => {
    const user = userEvent.setup();
    const { setConfig } = setupSettings({
      config: makeConfig({
        claude: {
          model: 'gpt-4o',
          max_tokens: 4096,
          system_prompt: '',
          fallback_model: undefined,
          api_key: undefined,
        },
      }),
      availableModels: [
        makeModel('gpt-4o', 'GPT-4o', 'OpenAI'),
        makeModel('claude-opus-4-6', 'Claude Opus 4.6', 'Anthropic'),
      ],
    });
    render(<ModelsTab />);

    await user.click(screen.getByRole('radio', { name: /claude-opus-4-6/i }));

    expect(setConfig).toHaveBeenCalledWith(
      expect.objectContaining({
        claude: expect.objectContaining({
          model: 'claude-opus-4-6',
          max_tokens: 16384,
        }),
      }),
    );
  });

  it('sets max_tokens to 16384 for gpt-4o model', async () => {
    const user = userEvent.setup();
    const { setConfig } = setupSettings({
      config: makeConfig({
        claude: {
          model: 'claude-opus-4-6',
          max_tokens: 16384,
          system_prompt: '',
          fallback_model: undefined,
          api_key: undefined,
        },
      }),
      availableModels: [
        makeModel('claude-opus-4-6', 'Claude Opus 4.6', 'Anthropic'),
        makeModel('gpt-4o', 'GPT-4o', 'OpenAI'),
      ],
    });
    render(<ModelsTab />);

    await user.click(screen.getByRole('radio', { name: /gpt-4o/i }));

    expect(setConfig).toHaveBeenCalledWith(
      expect.objectContaining({
        claude: expect.objectContaining({
          model: 'gpt-4o',
          max_tokens: 16384,
        }),
      }),
    );
  });
});

// ═══════════════════════════════════════════════════════════════════════════════
// 5. Fallback model
// ═══════════════════════════════════════════════════════════════════════════════

describe('Fallback model', () => {
  it('fallback select shows current fallback_model value', () => {
    setupSettings({
      config: makeConfig({
        claude: {
          model: 'claude-opus-4-6',
          fallback_model: 'gpt-4o',
          max_tokens: 8192,
          system_prompt: '',
          api_key: undefined,
        },
      }),
      availableModels: [
        makeModel('claude-opus-4-6', 'Claude Opus 4.6', 'Anthropic'),
        makeModel('gpt-4o', 'GPT-4o', 'OpenAI'),
      ],
    });
    render(<ModelsTab />);

    // Use the first combobox (fallback model select — may have multiple selects in the page)
    const selects = screen.getAllByRole('combobox');
    const select = selects.find(
      (s) => s.querySelector('option[value=""]') !== null,
    ) as HTMLSelectElement;
    expect(select.value).toBe('gpt-4o');
  });

  it('primary model is excluded from fallback options', () => {
    setupSettings({
      config: makeConfig({
        claude: {
          model: 'claude-opus-4-6',
          fallback_model: undefined,
          max_tokens: 8192,
          system_prompt: '',
          api_key: undefined,
        },
      }),
      availableModels: [
        makeModel('claude-opus-4-6', 'Claude Opus 4.6', 'Anthropic'),
        makeModel('gpt-4o', 'GPT-4o', 'OpenAI'),
      ],
    });
    render(<ModelsTab />);

    const selects = screen.getAllByRole('combobox');
    const select = selects.find((s) => s.querySelector('option[value=""]') !== null)!;
    const options = Array.from(select.querySelectorAll('option')).map(
      (o) => (o as HTMLOptionElement).value,
    );
    expect(options).not.toContain('claude-opus-4-6');
    expect(options).toContain('gpt-4o');
  });
});

// ═══════════════════════════════════════════════════════════════════════════════
// 6. Max Tokens
// ═══════════════════════════════════════════════════════════════════════════════

describe('Max Tokens', () => {
  it('shows current max_tokens value and calls setConfig on change', () => {
    const { setConfig } = setupSettings({
      config: makeConfig({
        claude: {
          model: 'claude-opus-4-6',
          max_tokens: 8192,
          system_prompt: '',
          fallback_model: undefined,
          api_key: undefined,
        },
      }),
    });
    render(<ModelsTab />);

    const input = screen.getByDisplayValue('8192') as HTMLInputElement;
    expect(input).toBeInTheDocument();
    expect(input.type).toBe('number');

    fireEvent.change(input, { target: { value: '4096' } });

    expect(setConfig).toHaveBeenCalledWith(
      expect.objectContaining({
        claude: expect.objectContaining({ max_tokens: 4096 }),
      }),
    );
  });
});

// ═══════════════════════════════════════════════════════════════════════════════
// 7. System Prompt
// ═══════════════════════════════════════════════════════════════════════════════

describe('System Prompt', () => {
  it('shows current system_prompt and calls setConfig on change', () => {
    const { setConfig } = setupSettings({
      config: makeConfig({
        claude: {
          model: 'claude-opus-4-6',
          max_tokens: 8192,
          system_prompt: 'You are NexiBot.',
          fallback_model: undefined,
          api_key: undefined,
        },
      }),
    });
    render(<ModelsTab />);

    const textarea = screen.getByDisplayValue('You are NexiBot.');
    expect(textarea.tagName).toBe('TEXTAREA');

    fireEvent.change(textarea, { target: { value: 'New prompt' } });

    expect(setConfig).toHaveBeenCalledWith(
      expect.objectContaining({
        claude: expect.objectContaining({ system_prompt: 'New prompt' }),
      }),
    );
  });
});

// ═══════════════════════════════════════════════════════════════════════════════
// 8. OpenAI API Key
// ═══════════════════════════════════════════════════════════════════════════════

describe('OpenAI API Key', () => {
  it('renders a password input for OpenAI API key and calls setConfig on change', () => {
    const { setConfig } = setupSettings();
    render(<ModelsTab />);

    // The OpenAI key input has placeholder "sk-..." and is rendered before DeepSeek
    // Both have the same placeholder; query by the OpenAI section heading proximity
    const allSkInputs = screen.getAllByPlaceholderText('sk-...');
    // OpenAI input is outside the provider collapsible section - it's the first one
    const openaiInput = allSkInputs[0] as HTMLInputElement;
    expect(openaiInput.type).toBe('password');

    fireEvent.change(openaiInput, { target: { value: 'sk-test123' } });

    expect(setConfig).toHaveBeenCalledWith(
      expect.objectContaining({
        openai: expect.objectContaining({ api_key: 'sk-test123' }),
      }),
    );
  });
});

// ═══════════════════════════════════════════════════════════════════════════════
// 9. Provider API Keys (CollapsibleSection)
// ═══════════════════════════════════════════════════════════════════════════════

describe('Provider API Keys', () => {
  it('renders Google Gemini API key input', () => {
    setupSettings();
    render(<ModelsTab />);

    const googleInput = screen.getByPlaceholderText('AIza...');
    expect(googleInput).toBeInTheDocument();
    expect((googleInput as HTMLInputElement).type).toBe('password');
  });

  it('calls setConfig when Gemini default model is changed', () => {
    const { setConfig } = setupSettings();
    render(<ModelsTab />);

    const geminiModelInput = screen.getAllByPlaceholderText('gemini-2.0-flash')[0];

    fireEvent.change(geminiModelInput, { target: { value: 'gemini-1.5-pro' } });

    expect(setConfig).toHaveBeenCalledWith(
      expect.objectContaining({
        google: expect.objectContaining({
          default_model: 'gemini-1.5-pro',
        }),
      }),
    );
  });

  it('renders DeepSeek API key input', () => {
    setupSettings();
    render(<ModelsTab />);

    // Both OpenAI and DeepSeek have sk-... placeholder; there should be at least 2
    const deepseekPlaceholders = screen.getAllByPlaceholderText('sk-...');
    expect(deepseekPlaceholders.length).toBeGreaterThanOrEqual(2);
  });

  it('renders GitHub Copilot token input', () => {
    setupSettings();
    render(<ModelsTab />);

    const copilotInput = screen.getByPlaceholderText('ghu_...');
    expect(copilotInput).toBeInTheDocument();
    expect((copilotInput as HTMLInputElement).type).toBe('password');
  });
});

// ═══════════════════════════════════════════════════════════════════════════════
// 10. Ollama
// ═══════════════════════════════════════════════════════════════════════════════

describe('Ollama', () => {
  it('renders "Discover Ollama Models" button', () => {
    setupSettings();
    render(<ModelsTab />);
    expect(
      screen.getByRole('button', { name: 'Discover Ollama Models' }),
    ).toBeInTheDocument();
  });

  it('clicking Discover Ollama Models invokes discover_ollama_models and shows alert', async () => {
    const user = userEvent.setup();
    vi.mocked(invoke).mockResolvedValueOnce(['llama3', 'mistral']);
    setupSettings();
    render(<ModelsTab />);

    await user.click(
      screen.getByRole('button', { name: 'Discover Ollama Models' }),
    );

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('discover_ollama_models');
    });

    await waitFor(() => {
      expect(vi.mocked(emit)).toHaveBeenCalledWith(
        'notify:toast',
        expect.objectContaining({ message: expect.stringContaining('llama3') }),
      );
    });
  });

  it('shows error alert when discover_ollama_models fails', async () => {
    const user = userEvent.setup();
    vi.mocked(invoke).mockRejectedValueOnce(new Error('connection refused'));
    setupSettings();
    render(<ModelsTab />);

    await user.click(
      screen.getByRole('button', { name: 'Discover Ollama Models' }),
    );

    await waitFor(() => {
      expect(vi.mocked(emit)).toHaveBeenCalledWith(
        'notify:toast',
        expect.objectContaining({ message: expect.stringContaining('Ollama not available') }),
      );
    });
  });
});
