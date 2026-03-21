import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { SkillsTab } from './SkillsTab';
import { invoke } from '@tauri-apps/api/core';
import { emit } from '@tauri-apps/api/event';
import { useSettings } from '../SettingsContext';

// -------------------------------------------------------------------
// Mocks
// -------------------------------------------------------------------
const mockConfirm = vi.hoisted(() => vi.fn(() => Promise.resolve(true)));
vi.mock('../../../shared/useConfirm', () => ({
  useConfirm: () => ({ confirm: mockConfirm, modal: null }),
}));
vi.mock('../SettingsContext');
vi.mock('../shared/InfoTip', () => ({ InfoTip: () => null }));
vi.mock('../../SkillMarketplace', () => ({
  SkillMarketplace: () => <div data-testid="skill-marketplace" />,
}));
// invoke is mocked globally via setup.ts

// -------------------------------------------------------------------
// Fixture helpers
// -------------------------------------------------------------------
function makeSkill(overrides: Partial<{
  id: string;
  metadata: Partial<{ name: string; description: string; user_invocable: boolean; command_dispatch: string }>;
  scripts: string[];
  content: string;
}> = {}) {
  return {
    id: overrides.id ?? 'test-skill',
    metadata: {
      name: 'Test Skill',
      description: 'A test skill',
      user_invocable: false,
      command_dispatch: 'inline',
      ...(overrides.metadata ?? {}),
    },
    scripts: overrides.scripts ?? [],
    content: overrides.content ?? '# Skill content',
  };
}

function makeTemplate(overrides: Partial<{
  id: string;
  name: string;
  description: string;
  content: string;
  user_invocable: boolean;
}> = {}) {
  return {
    id: overrides.id ?? 'template-skill',
    name: overrides.name ?? 'Template Skill',
    description: overrides.description ?? 'A template',
    content: overrides.content ?? '# Template',
    user_invocable: overrides.user_invocable ?? false,
  };
}

function defaultConfig() {
  return {
    timeout_seconds: 30,
    max_output_bytes: 1048576,
    values: {} as Record<string, string>,
  };
}

function setupSettings(
  skills: ReturnType<typeof makeSkill>[] = [],
  skillTemplates: ReturnType<typeof makeTemplate>[] = [],
) {
  const loadSkillsData = vi.fn();
  vi.mocked(useSettings).mockReturnValue({
    skills,
    skillTemplates,
    loadSkillsData,
  } as any);
  return { loadSkillsData };
}

// -------------------------------------------------------------------
// Global browser API stubs
// -------------------------------------------------------------------
beforeEach(() => {
  window.alert = vi.fn();
  window.confirm = vi.fn().mockReturnValue(true);
  vi.mocked(invoke).mockClear();
  vi.mocked(invoke).mockResolvedValue(undefined);
});

// ===================================================================
// 1. Empty state
// ===================================================================
describe('Empty state', () => {
  it('renders "No skills installed" when there are no skills', () => {
    setupSettings([], []);
    render(<SkillsTab />);
    expect(screen.getByText(/No skills installed/i)).toBeInTheDocument();
  });
});

// ===================================================================
// 2. Skill list rendering
// ===================================================================
describe('Skill list rendering', () => {
  it('renders skill name and description', () => {
    setupSettings([
      makeSkill({ metadata: { name: 'My Skill', description: 'Does things' } }),
    ]);
    render(<SkillsTab />);
    expect(screen.getByText('My Skill')).toBeInTheDocument();
    expect(screen.getByText('Does things')).toBeInTheDocument();
  });

  it('renders multiple skills', () => {
    setupSettings([
      makeSkill({ id: 'skill-a', metadata: { name: 'Skill A', description: 'Alpha' } }),
      makeSkill({ id: 'skill-b', metadata: { name: 'Skill B', description: 'Beta' } }),
    ]);
    render(<SkillsTab />);
    expect(screen.getByText('Skill A')).toBeInTheDocument();
    expect(screen.getByText('Skill B')).toBeInTheDocument();
  });
});

// ===================================================================
// 3. Configure badge — shown only for script-dispatched skills with scripts
// ===================================================================
describe('Configure badge', () => {
  it('shows "⚙ N scripts" badge for script skills with non-empty scripts array', () => {
    setupSettings([
      makeSkill({
        metadata: { command_dispatch: 'script' },
        scripts: ['run.sh', 'setup.sh'],
      }),
    ]);
    render(<SkillsTab />);
    expect(screen.getByText(/⚙ 2 scripts/i)).toBeInTheDocument();
  });

  it('does not show the badge for inline skills', () => {
    setupSettings([
      makeSkill({ metadata: { command_dispatch: 'inline' }, scripts: [] }),
    ]);
    render(<SkillsTab />);
    expect(screen.queryByText(/⚙/)).not.toBeInTheDocument();
  });

  it('does not show the badge for script skills with empty scripts array', () => {
    setupSettings([
      makeSkill({ metadata: { command_dispatch: 'script' }, scripts: [] }),
    ]);
    render(<SkillsTab />);
    expect(screen.queryByText(/⚙/)).not.toBeInTheDocument();
  });

  it('uses singular "script" when only 1 script present', () => {
    setupSettings([
      makeSkill({ metadata: { command_dispatch: 'script' }, scripts: ['run.sh'] }),
    ]);
    render(<SkillsTab />);
    expect(screen.getByText(/⚙ 1 script$/)).toBeInTheDocument();
  });
});

// ===================================================================
// 4. Configure button — shown only for executable skills
// ===================================================================
describe('Configure button visibility', () => {
  it('shows Configure button only for script skills with scripts', () => {
    setupSettings([
      makeSkill({ id: 'exec-skill', metadata: { command_dispatch: 'script' }, scripts: ['run.sh'] }),
    ]);
    render(<SkillsTab />);
    expect(screen.getByRole('button', { name: 'Configure' })).toBeInTheDocument();
  });

  it('does not show Configure button for inline skills', () => {
    setupSettings([
      makeSkill({ id: 'inline-skill', metadata: { command_dispatch: 'inline' }, scripts: [] }),
    ]);
    render(<SkillsTab />);
    expect(screen.queryByRole('button', { name: 'Configure' })).not.toBeInTheDocument();
  });

  it('does not show Configure button for script skill without scripts', () => {
    setupSettings([
      makeSkill({ id: 'no-scripts', metadata: { command_dispatch: 'script' }, scripts: [] }),
    ]);
    render(<SkillsTab />);
    expect(screen.queryByRole('button', { name: 'Configure' })).not.toBeInTheDocument();
  });
});

// ===================================================================
// 5. Configure opens editor
// ===================================================================
describe('Configure opens editor', () => {
  it('calls get_skill_config and shows config editor panel on click', async () => {
    const user = userEvent.setup();
    const skill = makeSkill({
      id: 'exec-skill',
      metadata: { name: 'Exec Skill', command_dispatch: 'script' },
      scripts: ['run.sh'],
    });
    setupSettings([skill]);
    vi.mocked(invoke).mockResolvedValue(defaultConfig());

    render(<SkillsTab />);
    await user.click(screen.getByRole('button', { name: 'Configure' }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('get_skill_config', { skillId: 'exec-skill' });
    });
    expect(screen.getByText(/Configure: Exec Skill/i)).toBeInTheDocument();
  });
});

// ===================================================================
// 6. Config editor fields
// ===================================================================
describe('Config editor fields', () => {
  async function openConfigEditor() {
    const user = userEvent.setup();
    const skill = makeSkill({
      id: 'exec-skill',
      metadata: { name: 'Exec Skill', command_dispatch: 'script' },
      scripts: ['run.sh'],
    });
    setupSettings([skill]);
    vi.mocked(invoke).mockResolvedValue(defaultConfig());
    render(<SkillsTab />);
    await user.click(screen.getByRole('button', { name: 'Configure' }));
    await waitFor(() => screen.getByText(/Configure: Exec Skill/i));
    return user;
  }

  it('shows timeout input with default value', async () => {
    await openConfigEditor();
    const timeoutInput = screen.getByDisplayValue('30');
    expect(timeoutInput).toBeInTheDocument();
  });

  it('shows max_output_bytes input with default value', async () => {
    await openConfigEditor();
    const maxBytesInput = screen.getByDisplayValue('1048576');
    expect(maxBytesInput).toBeInTheDocument();
  });

  it('shows no value pairs when values object is empty', async () => {
    await openConfigEditor();
    // The × remove button only appears for existing pairs
    expect(screen.queryByRole('button', { name: '×' })).not.toBeInTheDocument();
  });
});

// ===================================================================
// 7. Timeout input update
// ===================================================================
describe('Timeout input update', () => {
  it('updates the displayed timeout value when changed', async () => {
    const user = userEvent.setup();
    const skill = makeSkill({
      id: 'exec-skill',
      metadata: { command_dispatch: 'script' },
      scripts: ['run.sh'],
    });
    setupSettings([skill]);
    vi.mocked(invoke).mockResolvedValue(defaultConfig());
    render(<SkillsTab />);

    await user.click(screen.getByRole('button', { name: 'Configure' }));
    await waitFor(() => screen.getByDisplayValue('30'));

    const timeoutInput = screen.getByDisplayValue('30');
    fireEvent.change(timeoutInput, { target: { value: '60' } });

    expect(screen.getByDisplayValue('60')).toBeInTheDocument();
  });
});

// ===================================================================
// 8. Key-value add
// ===================================================================
describe('Key-value add', () => {
  it('adds a new key-value pair and clears the inputs after clicking Add', async () => {
    const user = userEvent.setup();
    const skill = makeSkill({
      id: 'exec-skill',
      metadata: { command_dispatch: 'script' },
      scripts: ['run.sh'],
    });
    setupSettings([skill]);
    vi.mocked(invoke).mockResolvedValue(defaultConfig());
    render(<SkillsTab />);

    await user.click(screen.getByRole('button', { name: 'Configure' }));
    await waitFor(() => screen.getByPlaceholderText('KEY_NAME'));

    const keyInput = screen.getByPlaceholderText('KEY_NAME');
    const valueInput = screen.getByPlaceholderText('value');

    await user.type(keyInput, 'MY_KEY');
    await user.type(valueInput, 'my_value');
    await user.click(screen.getByRole('button', { name: 'Add' }));

    // The key should now appear as a code label
    expect(screen.getByText('MY_KEY')).toBeInTheDocument();
    // Inputs should be cleared
    expect(keyInput).toHaveValue('');
    expect(valueInput).toHaveValue('');
  });
});

// ===================================================================
// 9. Key is uppercased
// ===================================================================
describe('Key uppercasing', () => {
  it('converts typed key to uppercase and strips invalid characters', async () => {
    const user = userEvent.setup();
    const skill = makeSkill({
      id: 'exec-skill',
      metadata: { command_dispatch: 'script' },
      scripts: ['run.sh'],
    });
    setupSettings([skill]);
    vi.mocked(invoke).mockResolvedValue(defaultConfig());
    render(<SkillsTab />);

    await user.click(screen.getByRole('button', { name: 'Configure' }));
    await waitFor(() => screen.getByPlaceholderText('KEY_NAME'));

    const keyInput = screen.getByPlaceholderText('KEY_NAME');
    await user.type(keyInput, 'my_key');

    expect(keyInput).toHaveValue('MY_KEY');
  });
});

// ===================================================================
// 10. Enter to add pair
// ===================================================================
describe('Enter to add pair', () => {
  it('adds the pair when Enter is pressed in the value input while key is non-empty', async () => {
    const user = userEvent.setup();
    const skill = makeSkill({
      id: 'exec-skill',
      metadata: { command_dispatch: 'script' },
      scripts: ['run.sh'],
    });
    setupSettings([skill]);
    vi.mocked(invoke).mockResolvedValue(defaultConfig());
    render(<SkillsTab />);

    await user.click(screen.getByRole('button', { name: 'Configure' }));
    await waitFor(() => screen.getByPlaceholderText('KEY_NAME'));

    const keyInput = screen.getByPlaceholderText('KEY_NAME');
    const valueInput = screen.getByPlaceholderText('value');

    await user.type(keyInput, 'API_KEY');
    await user.type(valueInput, 'secret123');
    await user.keyboard('{Enter}');

    expect(screen.getByText('API_KEY')).toBeInTheDocument();
  });
});

// ===================================================================
// 11. Remove key-value pair
// ===================================================================
describe('Remove key-value pair', () => {
  it('removes a pair when the × button is clicked', async () => {
    const user = userEvent.setup();
    const skill = makeSkill({
      id: 'exec-skill',
      metadata: { command_dispatch: 'script' },
      scripts: ['run.sh'],
    });
    setupSettings([skill]);
    vi.mocked(invoke).mockResolvedValue({
      ...defaultConfig(),
      values: { EXISTING_KEY: 'existing_value' },
    });
    render(<SkillsTab />);

    await user.click(screen.getByRole('button', { name: 'Configure' }));
    await waitFor(() => screen.getByText('EXISTING_KEY'));

    const removeButton = screen.getByRole('button', { name: '×' });
    await user.click(removeButton);

    expect(screen.queryByText('EXISTING_KEY')).not.toBeInTheDocument();
  });
});

// ===================================================================
// 12. Save config
// ===================================================================
describe('Save config', () => {
  it('calls save_skill_config with correct args and closes the editor', async () => {
    const user = userEvent.setup();
    const skill = makeSkill({
      id: 'exec-skill',
      metadata: { name: 'Exec Skill', command_dispatch: 'script' },
      scripts: ['run.sh'],
    });
    setupSettings([skill]);
    const config = defaultConfig();
    vi.mocked(invoke)
      .mockResolvedValueOnce(config)  // get_skill_config
      .mockResolvedValueOnce(undefined); // save_skill_config

    render(<SkillsTab />);
    await user.click(screen.getByRole('button', { name: 'Configure' }));
    await waitFor(() => screen.getByText(/Configure: Exec Skill/i));

    await user.click(screen.getByRole('button', { name: 'Save Config' }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('save_skill_config', {
        skillId: 'exec-skill',
        config,
      });
    });
    // Editor should close
    await waitFor(() => {
      expect(screen.queryByText(/Configure: Exec Skill/i)).not.toBeInTheDocument();
    });
  });
});

// ===================================================================
// 13. Cancel config
// ===================================================================
describe('Cancel config', () => {
  it('closes the config editor without calling save when Cancel is clicked', async () => {
    const user = userEvent.setup();
    const skill = makeSkill({
      id: 'exec-skill',
      metadata: { name: 'Exec Skill', command_dispatch: 'script' },
      scripts: ['run.sh'],
    });
    setupSettings([skill]);
    vi.mocked(invoke).mockResolvedValue(defaultConfig());

    render(<SkillsTab />);
    await user.click(screen.getByRole('button', { name: 'Configure' }));
    await waitFor(() => screen.getByText(/Configure: Exec Skill/i));

    const cancelButton = screen.getAllByRole('button', { name: 'Cancel' })[0];
    await user.click(cancelButton);

    expect(screen.queryByText(/Configure: Exec Skill/i)).not.toBeInTheDocument();
    expect(vi.mocked(invoke)).not.toHaveBeenCalledWith('save_skill_config', expect.anything());
  });
});

// ===================================================================
// 14. Save failure
// ===================================================================
describe('Save failure', () => {
  it('shows an alert when save_skill_config rejects', async () => {
    const user = userEvent.setup();
    const skill = makeSkill({
      id: 'exec-skill',
      metadata: { command_dispatch: 'script' },
      scripts: ['run.sh'],
    });
    setupSettings([skill]);
    vi.mocked(invoke)
      .mockResolvedValueOnce(defaultConfig())           // get_skill_config
      .mockRejectedValueOnce(new Error('disk full'));   // save_skill_config

    render(<SkillsTab />);
    await user.click(screen.getByRole('button', { name: 'Configure' }));
    await waitFor(() => screen.getByRole('button', { name: 'Save Config' }));

    await user.click(screen.getByRole('button', { name: 'Save Config' }));

    await waitFor(() => {
      expect(vi.mocked(emit)).toHaveBeenCalledWith(
        'notify:toast',
        expect.objectContaining({ message: expect.stringContaining('disk full') }),
      );
    });
  });
});

// ===================================================================
// 15. Test skill
// ===================================================================
describe('Test skill', () => {
  it('calls test_skill and shows result text', async () => {
    const user = userEvent.setup();
    setupSettings([makeSkill({ id: 'my-skill' })]);
    vi.mocked(invoke).mockResolvedValue('Test passed!');

    render(<SkillsTab />);
    await user.click(screen.getByRole('button', { name: 'Test' }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('test_skill', { skillId: 'my-skill' });
    });
    expect(screen.getByText('Test passed!')).toBeInTheDocument();
  });
});

// ===================================================================
// 16. Test skill error
// ===================================================================
describe('Test skill error', () => {
  it('shows error text in result display when test_skill rejects', async () => {
    const user = userEvent.setup();
    setupSettings([makeSkill({ id: 'bad-skill' })]);
    vi.mocked(invoke).mockRejectedValue(new Error('execution failed'));

    render(<SkillsTab />);
    await user.click(screen.getByRole('button', { name: 'Test' }));

    await waitFor(() => {
      expect(screen.getByText(/Error:.*execution failed/i)).toBeInTheDocument();
    });
  });
});

// ===================================================================
// 17. Security button
// ===================================================================
describe('Security button', () => {
  it('calls analyze_skill_security and shows severity badge', async () => {
    const user = userEvent.setup();
    setupSettings([makeSkill({ id: 'my-skill' })]);
    vi.mocked(invoke).mockResolvedValue({
      severity: 'LOW',
      findings: [],
      safe: true,
    });

    render(<SkillsTab />);
    await user.click(screen.getByRole('button', { name: 'Security' }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('analyze_skill_security', { skillId: 'my-skill' });
    });
    expect(screen.getByText('Safe')).toBeInTheDocument();
  });

  it('shows severity level badge when skill is not safe', async () => {
    const user = userEvent.setup();
    setupSettings([makeSkill({ id: 'risky-skill' })]);
    vi.mocked(invoke).mockResolvedValue({
      severity: 'HIGH',
      findings: [{ severity: 'HIGH', description: 'Suspicious network call' }],
      safe: false,
    });

    render(<SkillsTab />);
    await user.click(screen.getByRole('button', { name: 'Security' }));

    await waitFor(() => {
      expect(screen.getAllByText('HIGH').length).toBeGreaterThan(0);
    });
    expect(screen.getByText('Suspicious network call')).toBeInTheDocument();
  });
});

// ===================================================================
// 18. Delete with confirm
// ===================================================================
describe('Delete with confirm', () => {
  it('calls delete_skill and loadSkillsData when confirm returns true', async () => {
    const user = userEvent.setup();
    mockConfirm.mockResolvedValue(true);
    const { loadSkillsData } = setupSettings([makeSkill({ id: 'my-skill' })]);
    vi.mocked(invoke).mockResolvedValue(undefined);

    render(<SkillsTab />);
    await user.click(screen.getByRole('button', { name: 'Delete' }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('delete_skill', { skillId: 'my-skill' });
    });
    expect(loadSkillsData).toHaveBeenCalled();
  });
});

// ===================================================================
// 19. Delete cancelled
// ===================================================================
describe('Delete cancelled', () => {
  it('does NOT call delete_skill when confirm returns false', async () => {
    const user = userEvent.setup();
    mockConfirm.mockResolvedValue(false);
    setupSettings([makeSkill({ id: 'my-skill' })]);

    render(<SkillsTab />);
    await user.click(screen.getByRole('button', { name: 'Delete' }));

    expect(vi.mocked(invoke)).not.toHaveBeenCalledWith('delete_skill', expect.anything());
  });
});

// ===================================================================
// 20. Create Skill button
// ===================================================================
describe('Create Skill button', () => {
  it('shows the create form when "+ Create Skill" is clicked', async () => {
    const user = userEvent.setup();
    setupSettings([], []);
    render(<SkillsTab />);

    await user.click(screen.getByRole('button', { name: '+ Create Skill' }));

    expect(screen.getByText(/Create Skill/i)).toBeInTheDocument();
    expect(screen.getByPlaceholderText(/Skill ID/i)).toBeInTheDocument();
  });
});

// ===================================================================
// 21. Create Skill form validation
// ===================================================================
describe('Create Skill form validation', () => {
  it('keeps Save button disabled when Skill ID or Name is empty', async () => {
    const user = userEvent.setup();
    setupSettings([], []);
    render(<SkillsTab />);

    await user.click(screen.getByRole('button', { name: '+ Create Skill' }));

    const saveButton = screen.getByRole('button', { name: 'Save' });
    // Both empty — should be disabled
    expect(saveButton).toBeDisabled();

    // Fill ID only
    await user.type(screen.getByPlaceholderText(/Skill ID/i), 'my-skill');
    expect(saveButton).toBeDisabled();

    // Fill Name — now both are set, button should be enabled
    await user.type(screen.getByPlaceholderText(/Display name/i), 'My Skill');
    expect(saveButton).not.toBeDisabled();
  });
});

// ===================================================================
// 22. Create Skill save
// ===================================================================
describe('Create Skill save', () => {
  it('calls create_skill with correct args when form is saved', async () => {
    const user = userEvent.setup();
    const { loadSkillsData } = setupSettings([], []);
    vi.mocked(invoke).mockResolvedValue(undefined);
    render(<SkillsTab />);

    await user.click(screen.getByRole('button', { name: '+ Create Skill' }));

    await user.type(screen.getByPlaceholderText(/Skill ID/i), 'new-skill');
    await user.type(screen.getByPlaceholderText(/Display name/i), 'New Skill');
    await user.type(screen.getByPlaceholderText(/Description/i), 'A brand new skill');

    await user.click(screen.getByRole('button', { name: 'Save' }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('create_skill', expect.objectContaining({
        id: 'new-skill',
        name: 'New Skill',
        description: 'A brand new skill',
      }));
    });
    expect(loadSkillsData).toHaveBeenCalled();
  });
});

// ===================================================================
// 23. Edit skill
// ===================================================================
describe('Edit skill', () => {
  it('shows edit form pre-populated with skill values when Edit is clicked', async () => {
    const user = userEvent.setup();
    setupSettings([
      makeSkill({
        id: 'edit-me',
        metadata: { name: 'Edit Me', description: 'Editable skill' },
        content: '# Edit content',
      }),
    ]);
    render(<SkillsTab />);

    await user.click(screen.getByRole('button', { name: 'Edit' }));

    expect(screen.getByText(/Edit: Edit Me/i)).toBeInTheDocument();
    expect(screen.getByDisplayValue('Edit Me')).toBeInTheDocument();
    expect(screen.getByDisplayValue('Editable skill')).toBeInTheDocument();
    expect(screen.getByDisplayValue('# Edit content')).toBeInTheDocument();
  });
});

// ===================================================================
// 24. Reload Skills
// ===================================================================
describe('Reload Skills', () => {
  it('calls reload_skills and loadSkillsData when Reload Skills is clicked', async () => {
    const user = userEvent.setup();
    const { loadSkillsData } = setupSettings([], []);
    vi.mocked(invoke).mockResolvedValue(undefined);
    render(<SkillsTab />);

    await user.click(screen.getByRole('button', { name: 'Reload Skills' }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('reload_skills');
    });
    expect(loadSkillsData).toHaveBeenCalled();
  });
});

// ===================================================================
// 25. Template list
// ===================================================================
describe('Template list', () => {
  it('renders templates not yet installed with a "+ Add" button', () => {
    setupSettings(
      [makeSkill({ id: 'installed-skill' })],
      [
        makeTemplate({ id: 'template-one', name: 'Template One' }),
        makeTemplate({ id: 'installed-skill', name: 'Already Installed' }),
      ],
    );
    render(<SkillsTab />);

    expect(screen.getByText('Template One')).toBeInTheDocument();
    expect(screen.queryByText('Already Installed')).not.toBeInTheDocument();

    const addButtons = screen.getAllByRole('button', { name: '+ Add' });
    expect(addButtons).toHaveLength(1);
  });
});

// ===================================================================
// 26. Install template
// ===================================================================
describe('Install template', () => {
  it('calls create_skill and loadSkillsData when "+ Add" is clicked', async () => {
    const user = userEvent.setup();
    const template = makeTemplate({ id: 'tpl-skill', name: 'Tpl Skill', description: 'A template', content: '# Tpl', user_invocable: true });
    const { loadSkillsData } = setupSettings([], [template]);
    vi.mocked(invoke).mockResolvedValue(undefined);
    render(<SkillsTab />);

    await user.click(screen.getByRole('button', { name: '+ Add' }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('create_skill', {
        id: 'tpl-skill',
        name: 'Tpl Skill',
        description: 'A template',
        content: '# Tpl',
        userInvocable: true,
      });
    });
    expect(loadSkillsData).toHaveBeenCalled();
  });
});
