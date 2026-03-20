import { useState } from 'react';
import '../Settings.css';
import { SettingsProvider, useSettings } from './SettingsContext';
import { useConfirm } from '../../shared/useConfirm';
import { ModelsTab } from './tabs/ModelsTab';
import { VoiceTab } from './tabs/VoiceTab';
import { KnowledgeTab } from './tabs/KnowledgeTab';
import { ChannelsTab } from './tabs/ChannelsTab';
import { ToolsTab } from './tabs/ToolsTab';
import { AutomationTab } from './tabs/AutomationTab';
import { SkillsTab } from './tabs/SkillsTab';
import { SecurityTab } from './tabs/SecurityTab';
import { AgentsTab } from './tabs/AgentsTab';
import { SystemTab } from './tabs/SystemTab';
import { ConnectorsTab } from './tabs/ConnectorsTab';
import { KeyVaultTab } from './tabs/KeyVaultTab';
import { GatedShellTab } from './tabs/GatedShellTab';

type Tab = 'models' | 'voice' | 'knowledge' | 'channels' | 'tools' | 'connectors' | 'automation' | 'skills' | 'security' | 'vault' | 'agents' | 'system' | 'gated-shell';

const TABS: [Tab, string][] = [
  ['models', 'Models'],
  ['voice', 'Voice'],
  ['knowledge', 'Knowledge'],
  ['channels', 'Channels'],
  ['tools', 'Tools'],
  ['connectors', 'Connectors'],
  ['automation', 'Automation'],
  ['skills', 'Skills'],
  ['security', 'Security'],
  ['vault', 'Key Vault'],
  ['agents', 'Agents'],
  ['system', 'System'],
  ['gated-shell', 'NexiGate'],
];

interface SettingsProps {
  onClose: () => void;
}

function SettingsInner({ onClose }: SettingsProps) {
  const { config, isSaving, saveMessage, saveConfig, loadError, hasUnsavedChanges } = useSettings();
  const [activeTab, setActiveTab] = useState<Tab>('models');
  const { confirm: showConfirm, modal: confirmModal } = useConfirm();

  if (!config) {
    return (
      <div className="settings">
        <div style={{ padding: '20px', color: loadError ? 'var(--error)' : 'var(--text)' }}>
          {loadError || 'Loading settings...'}
        </div>
      </div>
    );
  }

  return (
    <div className="settings">
      {confirmModal}
      <div className="settings-header">
        <h2>Settings</h2>
        <div className="settings-header-actions">
          {saveMessage && <span className={`save-message ${saveMessage.startsWith('Failed') ? 'error' : ''}`}>{saveMessage}</span>}
          <button className="save-btn" onClick={saveConfig} disabled={isSaving}>
            {isSaving ? 'Saving...' : 'Save'}
          </button>
          <button
            className="close-btn"
            onClick={async () => {
              if (hasUnsavedChanges && !await showConfirm('You have unsaved changes. Close without saving?', { confirmLabel: 'Close anyway' })) {
                return;
              }
              onClose();
            }}
          >
            Done
          </button>
        </div>
      </div>

      <div className="tabs">
        {TABS.map(([key, label]) => (
          <button
            key={key}
            className={`tab ${activeTab === key ? 'active' : ''}`}
            onClick={() => setActiveTab(key)}
          >
            {label}
          </button>
        ))}
      </div>

      {activeTab === 'models' && <ModelsTab />}
      {activeTab === 'voice' && <VoiceTab />}
      {activeTab === 'knowledge' && <KnowledgeTab />}
      {activeTab === 'channels' && <ChannelsTab />}
      {activeTab === 'tools' && <ToolsTab />}
      {activeTab === 'connectors' && <ConnectorsTab />}
      {activeTab === 'automation' && <AutomationTab />}
      {activeTab === 'skills' && <SkillsTab />}
      {activeTab === 'security' && <SecurityTab />}
      {activeTab === 'vault' && <KeyVaultTab />}
      {activeTab === 'agents' && <AgentsTab />}
      {activeTab === 'system' && <SystemTab />}
      {activeTab === 'gated-shell' && <GatedShellTab />}
    </div>
  );
}

export default function Settings({ onClose }: SettingsProps) {
  return (
    <SettingsProvider>
      <SettingsInner onClose={onClose} />
    </SettingsProvider>
  );
}
