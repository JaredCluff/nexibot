import { useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useSettings } from '../SettingsContext';
import { InfoTip } from '../shared/InfoTip';
import { notifyError } from '../../../shared/notify';
import { useConfirm } from '../../../shared/useConfirm';
import { SkillMarketplace } from '../../SkillMarketplace';

type EditingSkill = {
  id: string;
  name: string;
  description: string;
  content: string;
  user_invocable: boolean;
  isNew: boolean;
} | null;

interface SkillConfig {
  timeout_seconds: number;
  max_output_bytes: number;
  values: Record<string, string>;
}

type ConfiguringSkill = {
  id: string;
  name: string;
  config: SkillConfig;
  newKey: string;
  newValue: string;
} | null;


export function SkillsTab() {
  const { skills, skillTemplates, loadSkillsData } = useSettings();
  const { confirm: showConfirm, modal: confirmModal } = useConfirm();

  const [editingSkill, setEditingSkill] = useState<EditingSkill>(null);
  const [testingSkill, setTestingSkill] = useState<string | null>(null);
  const [skillTestResult, setSkillTestResult] = useState<string | null>(null);
  const [reloadingSkills, setReloadingSkills] = useState(false);
  const [resettingSkills, setResettingSkills] = useState(false);

  const [deletingSkillId, setDeletingSkillId] = useState<string | null>(null);

  // Security analysis state
  const [analyzingSkillId, setAnalyzingSkillId] = useState<string | null>(null);
  const [skillSecurityReports, setSkillSecurityReports] = useState<Record<string, { severity: string; findings: { severity: string; description: string }[]; safe: boolean }>>({});

  // Per-skill config editor state
  const [configuringSkill, setConfiguringSkill] = useState<ConfiguringSkill>(null);
  const [savingConfig, setSavingConfig] = useState(false);

  const openConfigEditor = async (skill: any) => {
    try {
      const config = await invoke<SkillConfig>('get_skill_config', { skillId: skill.id });
      setConfiguringSkill({
        id: skill.id,
        name: skill.metadata?.name || skill.id,
        config,
        newKey: '',
        newValue: '',
      });
    } catch (error) {
      notifyError('Skills', `Failed to load config: ${error}`);
    }
  };

  const saveConfig = async () => {
    if (!configuringSkill) return;
    setSavingConfig(true);
    try {
      await invoke('save_skill_config', {
        skillId: configuringSkill.id,
        config: configuringSkill.config,
      });
      setConfiguringSkill(null);
    } catch (error) {
      notifyError('Skills', `Failed to save config: ${error}`);
    } finally {
      setSavingConfig(false);
    }
  };

  // installed slugs set derived from skills list (for SkillMarketplace)
  const installedSlugSet = new Set(skills.map(s => s.id));

  return (
    <div className="tab-content">
      {confirmModal}
      <div className="settings-group">
        <h3>Installed Skills<InfoTip text="Custom capabilities that extend NexiBot. Skills inject specialized instructions into the system prompt." /></h3>
        <div className="action-buttons">
          <button onClick={async () => {
            setReloadingSkills(true);
            try {
              await invoke('reload_skills');
              loadSkillsData();
            } catch (error) {
              notifyError('Skills', `Reload failed: ${error}`);
            } finally {
              setReloadingSkills(false);
            }
          }} disabled={reloadingSkills}>
            {reloadingSkills ? 'Loading...' : 'Reload Skills'}
          </button>
          <button className="danger" onClick={async () => {
            if (!await showConfirm('Reset all skills to defaults? This will remove any custom skills.', { danger: true, confirmLabel: 'Reset' })) return;
            setResettingSkills(true);
            try {
              await invoke('reset_bundled_skills');
              loadSkillsData();
            } catch (error) {
              notifyError('Skills', `Reset failed: ${error}`);
            } finally {
              setResettingSkills(false);
            }
          }} disabled={resettingSkills}>
            {resettingSkills ? 'Loading...' : 'Reset to Defaults'}
          </button>
        </div>
        <p className="group-description">
          Skills extend NexiBot with custom capabilities. User-invocable skills can be triggered with /commands.
        </p>

        {skills.length === 0 && !editingSkill && (
          <div className="info-text">No skills installed. Create one or use a template below.</div>
        )}

        {skills.map((skill) => (
          <div key={skill.id} className="mcp-server-card">
            <div className="mcp-server-header">
              <span className="mcp-server-name">{skill.metadata?.name || skill.id}</span>
              {skill.metadata?.user_invocable && (
                <span className="mcp-tool-count">/{skill.id}</span>
              )}
              {skill.metadata?.command_dispatch === 'script' && skill.scripts?.length > 0 && (
                <span className="mcp-tool-count" title={`Scripts: ${skill.scripts.join(', ')}`}>
                  ⚙ {skill.scripts.length} script{skill.scripts.length !== 1 ? 's' : ''}
                </span>
              )}
              <span className="mcp-server-command">
                {skill.metadata?.description || 'No description'}
              </span>
            </div>
            <div className="action-buttons">
              <button onClick={() => setEditingSkill({
                id: skill.id,
                name: skill.metadata?.name || skill.id,
                description: skill.metadata?.description || '',
                content: skill.content || '',
                user_invocable: skill.metadata?.user_invocable ?? true,
                isNew: false,
              })}>Edit</button>
              {skill.metadata?.command_dispatch === 'script' && skill.scripts?.length > 0 && (
                <button onClick={() => openConfigEditor(skill)}>Configure</button>
              )}
              <button className="primary" disabled={testingSkill === skill.id} onClick={async () => {
                setTestingSkill(skill.id);
                setSkillTestResult(null);
                try {
                  const result = await invoke<string>('test_skill', { skillId: skill.id });
                  setSkillTestResult(result);
                } catch (error) {
                  setSkillTestResult(`Error: ${error}`);
                } finally {
                  setTestingSkill(null);
                }
              }}>{testingSkill === skill.id ? 'Testing...' : 'Test'}</button>
              <button disabled={analyzingSkillId === skill.id} onClick={async () => {
                setAnalyzingSkillId(skill.id);
                try {
                  const report = await invoke<{ severity: string; findings: { severity: string; description: string }[]; safe: boolean }>('analyze_skill_security', { skillId: skill.id });
                  setSkillSecurityReports(prev => ({ ...prev, [skill.id]: report }));
                } catch (error) {
                  notifyError('Skills', `Analysis failed: ${error}`);
                } finally {
                  setAnalyzingSkillId(null);
                }
              }}>{analyzingSkillId === skill.id ? 'Analyzing...' : 'Security'}</button>
              <button className="danger" disabled={deletingSkillId === skill.id} onClick={async () => {
                if (!await showConfirm(`Delete skill "${skill.metadata?.name || skill.id}"?`, { danger: true, confirmLabel: 'Delete' })) return;
                setDeletingSkillId(skill.id);
                try {
                  await invoke('delete_skill', { skillId: skill.id });
                  loadSkillsData();
                } catch (error) {
                  notifyError('Skills', `Failed to delete skill: ${error}`);
                } finally {
                  setDeletingSkillId(null);
                }
              }}>{deletingSkillId === skill.id ? 'Deleting…' : 'Delete'}</button>
            </div>
            {skillSecurityReports[skill.id] && (
              <div style={{ margin: '8px 0', fontSize: '12px' }}>
                <span className={`severity-badge severity-${skillSecurityReports[skill.id].severity.toLowerCase()}`}>
                  {skillSecurityReports[skill.id].safe ? 'Safe' : skillSecurityReports[skill.id].severity}
                </span>
                {skillSecurityReports[skill.id].findings.length > 0 && (
                  <ul style={{ margin: '4px 0 0 16px', padding: 0 }}>
                    {skillSecurityReports[skill.id].findings.map((f, i) => (
                      <li key={i} style={{ color: 'var(--text-secondary)' }}>
                        <span className={`severity-badge severity-${f.severity.toLowerCase()}`} style={{ marginRight: '4px' }}>{f.severity}</span>
                        {f.description}
                      </li>
                    ))}
                  </ul>
                )}
              </div>
            )}
          </div>
        ))}

        {skillTestResult && (
          <div className="mcp-server-card">
            <div style={{ fontSize: '12px', color: 'var(--text-secondary)', whiteSpace: 'pre-wrap', maxHeight: '150px', overflow: 'auto' }}>
              {skillTestResult}
            </div>
            <button className="test-button" onClick={() => setSkillTestResult(null)} style={{ marginTop: '4px' }}>
              Dismiss
            </button>
          </div>
        )}

        {!editingSkill && (
          <button className="mcp-add-btn" onClick={() => setEditingSkill({
            id: '', name: '', description: '', content: '', user_invocable: true, isNew: true,
          })}>
            + Create Skill
          </button>
        )}
      </div>

      {configuringSkill && (
        <div className="settings-group">
          <h3>Configure: {configuringSkill.name}</h3>
          <p className="group-description">
            Runtime configuration for this skill's scripts. Values are injected as <code>SKILL_CONFIG_&lt;KEY&gt;</code> environment variables.
          </p>

          <label className="field">
            <span>Timeout (seconds)<InfoTip text="Maximum time a script is allowed to run before being killed." /></span>
            <input
              type="number"
              min={1}
              max={300}
              value={configuringSkill.config.timeout_seconds}
              onChange={(e) => setConfiguringSkill({
                ...configuringSkill,
                config: { ...configuringSkill.config, timeout_seconds: parseInt(e.target.value) || 30 },
              })}
            />
          </label>

          <label className="field">
            <span>Max output size (bytes)<InfoTip text="Maximum script output captured. Output beyond this limit is truncated." /></span>
            <input
              type="number"
              min={1024}
              max={10485760}
              step={1024}
              value={configuringSkill.config.max_output_bytes}
              onChange={(e) => setConfiguringSkill({
                ...configuringSkill,
                config: { ...configuringSkill.config, max_output_bytes: parseInt(e.target.value) || 1048576 },
              })}
            />
          </label>

          <div className="field">
            <span>Config values<InfoTip text="Key-value pairs injected as SKILL_CONFIG_<KEY> environment variables. Keys must be uppercase letters, digits, and underscores only." /></span>
            {Object.keys(configuringSkill.config.values).length > 0 && (
              <div style={{ marginTop: '6px' }}>
                {Object.entries(configuringSkill.config.values).map(([key, value]) => (
                  <div key={key} style={{ display: 'flex', gap: '8px', marginBottom: '4px', alignItems: 'center' }}>
                    <code style={{ minWidth: '160px', padding: '4px 6px', background: 'var(--bg-tertiary)', borderRadius: '4px', fontSize: '12px' }}>
                      {key}
                    </code>
                    <input
                      type="text"
                      value={value}
                      style={{ flex: 1 }}
                      onChange={(e) => {
                        const newValues = { ...configuringSkill.config.values, [key]: e.target.value };
                        setConfiguringSkill({ ...configuringSkill, config: { ...configuringSkill.config, values: newValues } });
                      }}
                    />
                    <button
                      className="danger"
                      style={{ padding: '4px 8px', fontSize: '12px' }}
                      onClick={() => {
                        const newValues = { ...configuringSkill.config.values };
                        delete newValues[key];
                        setConfiguringSkill({ ...configuringSkill, config: { ...configuringSkill.config, values: newValues } });
                      }}
                    >×</button>
                  </div>
                ))}
              </div>
            )}
            <div style={{ display: 'flex', gap: '8px', marginTop: '8px', alignItems: 'center' }}>
              <input
                type="text"
                placeholder="KEY_NAME"
                value={configuringSkill.newKey}
                style={{ width: '160px', fontFamily: 'monospace' }}
                onChange={(e) => setConfiguringSkill({
                  ...configuringSkill,
                  newKey: e.target.value.toUpperCase().replace(/[^A-Z0-9_]/g, ''),
                })}
              />
              <input
                type="text"
                placeholder="value"
                value={configuringSkill.newValue}
                style={{ flex: 1 }}
                onChange={(e) => setConfiguringSkill({ ...configuringSkill, newValue: e.target.value })}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' && configuringSkill.newKey) {
                    const newValues = { ...configuringSkill.config.values, [configuringSkill.newKey]: configuringSkill.newValue };
                    setConfiguringSkill({ ...configuringSkill, config: { ...configuringSkill.config, values: newValues }, newKey: '', newValue: '' });
                  }
                }}
              />
              <button
                disabled={!configuringSkill.newKey}
                onClick={() => {
                  const newValues = { ...configuringSkill.config.values, [configuringSkill.newKey]: configuringSkill.newValue };
                  setConfiguringSkill({ ...configuringSkill, config: { ...configuringSkill.config, values: newValues }, newKey: '', newValue: '' });
                }}
              >Add</button>
            </div>
          </div>

          <div className="mcp-add-actions">
            <button className="primary" disabled={savingConfig} onClick={saveConfig}>
              {savingConfig ? 'Saving...' : 'Save Config'}
            </button>
            <button onClick={() => setConfiguringSkill(null)}>Cancel</button>
          </div>
        </div>
      )}

      {editingSkill && (
        <div className="settings-group">
          <h3>{editingSkill.isNew ? 'Create Skill' : `Edit: ${editingSkill.name}`}</h3>
          {editingSkill.isNew && (
            <label className="field">
              <span>Skill ID<InfoTip text="Unique identifier for this skill. Used as the /command name if user-invocable. Use lowercase with hyphens." /></span>
              <input
                type="text"
                placeholder="Skill ID (e.g., code-review)"
                value={editingSkill.id}
                onChange={(e) => setEditingSkill({ ...editingSkill, id: e.target.value.toLowerCase().replace(/[^a-z0-9-]/g, '-') })}
              />
            </label>
          )}
          <label className="field">
            <span>Display Name<InfoTip text="The human-readable name shown in the skills list." /></span>
            <input
              type="text"
              placeholder="Display name"
              value={editingSkill.name}
              onChange={(e) => setEditingSkill({ ...editingSkill, name: e.target.value })}
            />
          </label>
          <label className="field">
            <span>Description<InfoTip text="A brief description of what this skill does." /></span>
            <input
              type="text"
              placeholder="Description"
              value={editingSkill.description}
              onChange={(e) => setEditingSkill({ ...editingSkill, description: e.target.value })}
            />
          </label>
          <div className="inline-toggle">
            <label className="toggle-label">
              <input
                type="checkbox"
                checked={editingSkill.user_invocable}
                onChange={(e) => setEditingSkill({ ...editingSkill, user_invocable: e.target.checked })}
              />
              User-invocable (available as /command)<InfoTip text="When enabled, users can trigger this skill by typing /{skill-id} in the chat." />
            </label>
          </div>
          <label className="field">
            <span>Skill Content<InfoTip text="The skill instructions in markdown. These are injected into the system prompt when the skill is active." /></span>
            <textarea
              placeholder="Skill instructions (markdown) — these are injected into the system prompt"
              value={editingSkill.content}
              onChange={(e) => setEditingSkill({ ...editingSkill, content: e.target.value })}
              rows={10}
            />
          </label>
          <div className="mcp-add-actions">
            <button
              className="primary"
              disabled={!editingSkill.id || !editingSkill.name}
              onClick={async () => {
                try {
                  if (editingSkill.isNew) {
                    await invoke('create_skill', {
                      id: editingSkill.id,
                      name: editingSkill.name,
                      description: editingSkill.description,
                      content: editingSkill.content,
                      userInvocable: editingSkill.user_invocable,
                    });
                  } else {
                    await invoke('update_skill', {
                      id: editingSkill.id,
                      name: editingSkill.name,
                      description: editingSkill.description,
                      content: editingSkill.content,
                      userInvocable: editingSkill.user_invocable,
                    });
                  }
                  setEditingSkill(null);
                  loadSkillsData();
                } catch (error) {
                  notifyError('Skills', `Failed to save skill: ${error}`);
                }
              }}
            >Save</button>
            <button onClick={() => setEditingSkill(null)}>Cancel</button>
          </div>
        </div>
      )}

      {skillTemplates.length > 0 && (
        <div className="settings-group">
          <h3>Templates</h3>
          <p className="group-description">One-click create from preset templates.</p>
          <div className="mcp-presets">
            {skillTemplates.filter(t => !skills.some(s => s.id === t.id)).map((template) => (
              <div key={template.id} className="mcp-preset-card" onClick={async () => {
                try {
                  await invoke('create_skill', {
                    id: template.id,
                    name: template.name,
                    description: template.description,
                    content: template.content,
                    userInvocable: template.user_invocable,
                  });
                  loadSkillsData();
                } catch (error) {
                  notifyError('Skills', `Failed to create from template: ${error}`);
                }
              }}>
                <div className="mcp-preset-info">
                  <span className="mcp-server-name">{template.name}</span>
                  <span className="mcp-tool-desc">{template.description}</span>
                </div>
                <button className="mcp-toggle-btn">+ Add</button>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* ClawHub Marketplace */}
      <div className="settings-group">
        <h3>ClawHub Marketplace<InfoTip text="Browse and install community-created skills from ClawHub. Skills are scanned for security before installation." /></h3>
        <p className="group-description">
          Discover and install skills from the ClawHub community marketplace.
        </p>
        <SkillMarketplace
          installedSlugs={installedSlugSet}
          onInstalled={() => loadSkillsData()}
        />
      </div>
    </div>
  );
}
