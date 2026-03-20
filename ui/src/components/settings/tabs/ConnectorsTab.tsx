import { useState, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useSettings } from '../SettingsContext';
import { InfoTip } from '../shared/InfoTip';
import { CollapsibleSection } from '../shared/CollapsibleSection';
import { notifyError } from '../../../shared/notify';
import { ConnectorWizard } from '../../ConnectorWizard';

// ─── Types ───────────────────────────────────────────────────────────────────

interface IntegrationCredentialInfo {
  service: string;
  key_name: string;
  scope: string;
  label: string;
  stored_at: string;
}

interface ServiceField {
  key: string;
  label: string;
  type: 'text' | 'password';
  placeholder: string;
  required: boolean;
}

interface ServiceScope {
  id: string;
  label: string;
  description: string;
}

interface ServiceDefinition {
  id: string;
  name: string;
  description: string;
  fields: ServiceField[];
  scopes: ServiceScope[];
  setupGuide: string;
  docsUrl: string;
  skillId: string;
}

interface K2KCapability {
  id: string;
  name: string;
  category: string;
  description: string;
  version: string;
}

// ─── Service Definitions ─────────────────────────────────────────────────────

const SERVICE_DEFINITIONS: ServiceDefinition[] = [
  {
    id: 'clickup',
    name: 'ClickUp',
    description: 'Task management and project tracking',
    fields: [
      { key: 'api_key', label: 'API Key', type: 'password', placeholder: 'pk_...', required: true },
    ],
    scopes: [
      { id: 'readonly', label: 'Read-only', description: 'List and search tasks, spaces, lists' },
      { id: 'full', label: 'Full Access', description: 'Create, update, delete tasks and manage workspaces' },
    ],
    setupGuide: 'Go to ClickUp Settings > Apps > Generate API Token',
    docsUrl: 'https://clickup.com/api',
    skillId: 'clickup',
  },
  {
    id: 'google-workspace',
    name: 'Google Workspace',
    description: 'Google Drive, Docs, Calendar, and Gmail',
    fields: [
      { key: 'client_id', label: 'Client ID', type: 'text', placeholder: '...apps.googleusercontent.com', required: true },
      { key: 'client_secret', label: 'Client Secret', type: 'password', placeholder: 'GOCSPX-...', required: true },
      { key: 'refresh_token', label: 'Refresh Token', type: 'password', placeholder: '1//...', required: true },
    ],
    scopes: [
      { id: 'readonly', label: 'Read-only', description: 'List and search files, read calendar events, read emails' },
      { id: 'readwrite', label: 'Read-Write', description: 'Create and edit documents, manage calendar events, send emails' },
    ],
    setupGuide: 'Create OAuth credentials at console.cloud.google.com, then use the OAuth playground to get a refresh token',
    docsUrl: 'https://developers.google.com/workspace',
    skillId: 'google-workspace',
  },
  {
    id: 'atlassian',
    name: 'Atlassian (Jira + Confluence)',
    description: 'Issue tracking and documentation',
    fields: [
      { key: 'email', label: 'Email', type: 'text', placeholder: 'you@company.com', required: true },
      { key: 'api_token', label: 'API Token', type: 'password', placeholder: 'ATATT3xF...', required: true },
      { key: 'domain', label: 'Domain', type: 'text', placeholder: 'yourcompany (without .atlassian.net)', required: true },
    ],
    scopes: [
      { id: 'readonly', label: 'Read-only', description: 'Search and read issues, pages, projects' },
      { id: 'readwrite', label: 'Read-Write', description: 'Create and update issues, create and edit pages' },
    ],
    setupGuide: 'Go to id.atlassian.com > Security > API Tokens > Create API Token',
    docsUrl: 'https://developer.atlassian.com/cloud/jira/platform/rest/v3/',
    skillId: 'atlassian',
  },
  {
    id: 'servicenow',
    name: 'ServiceNow',
    description: 'IT service management and incident tracking',
    fields: [
      { key: 'instance_url', label: 'Instance URL', type: 'text', placeholder: 'https://yourinstance.service-now.com', required: true },
      { key: 'username', label: 'Username', type: 'text', placeholder: 'admin', required: true },
      { key: 'password', label: 'Password', type: 'password', placeholder: 'Password', required: true },
    ],
    scopes: [
      { id: 'readonly', label: 'Read-only', description: 'Search and read incidents, requests, knowledge articles' },
      { id: 'readwrite', label: 'Read-Write', description: 'Create and update incidents, manage service requests' },
    ],
    setupGuide: 'Use your ServiceNow instance credentials. Consider creating a dedicated integration user.',
    docsUrl: 'https://developer.servicenow.com/dev.do',
    skillId: 'servicenow',
  },
  {
    id: 'salesforce',
    name: 'Salesforce',
    description: 'CRM, leads, opportunities, and cases',
    fields: [
      { key: 'client_id', label: 'Client ID', type: 'text', placeholder: 'Connected App Consumer Key', required: true },
      { key: 'client_secret', label: 'Client Secret', type: 'password', placeholder: 'Connected App Consumer Secret', required: true },
      { key: 'refresh_token', label: 'Refresh Token', type: 'password', placeholder: 'OAuth refresh token', required: true },
      { key: 'instance_url', label: 'Instance URL', type: 'text', placeholder: 'https://yourorg.my.salesforce.com', required: true },
    ],
    scopes: [
      { id: 'readonly', label: 'Read-only', description: 'Query and read records, leads, opportunities, cases' },
      { id: 'readwrite', label: 'Read-Write', description: 'Create and update records, manage leads and opportunities' },
    ],
    setupGuide: 'Create a Connected App in Salesforce Setup, enable OAuth, and generate a refresh token',
    docsUrl: 'https://developer.salesforce.com/docs/atlas.en-us.api_rest.meta/api_rest/',
    skillId: 'salesforce',
  },
  {
    id: 'monday',
    name: 'Monday.com',
    description: 'Work management, boards, and items',
    fields: [
      { key: 'api_key', label: 'API Key', type: 'password', placeholder: 'API v2 Token', required: true },
    ],
    scopes: [
      { id: 'readonly', label: 'Read-only', description: 'List and search boards, items, columns' },
      { id: 'readwrite', label: 'Read-Write', description: 'Create and update items, add updates, manage boards' },
    ],
    setupGuide: 'Go to Monday.com > Profile > Admin > API > Personal API Token',
    docsUrl: 'https://developer.monday.com/api-reference/',
    skillId: 'monday',
  },
  {
    id: 'microsoft365',
    name: 'Microsoft 365',
    description: 'Outlook, Calendar, OneDrive, and Teams',
    fields: [
      { key: 'client_id', label: 'Client ID', type: 'text', placeholder: 'Application (client) ID', required: true },
      { key: 'client_secret', label: 'Client Secret', type: 'password', placeholder: 'Client secret value', required: true },
      { key: 'tenant_id', label: 'Tenant ID', type: 'text', placeholder: 'Directory (tenant) ID', required: true },
      { key: 'refresh_token', label: 'Refresh Token', type: 'password', placeholder: 'OAuth refresh token', required: true },
    ],
    scopes: [
      { id: 'readonly', label: 'Read-only', description: 'Read mail, calendar events, files, and messages' },
      { id: 'readwrite', label: 'Read-Write', description: 'Send mail, create events, upload files, post messages' },
    ],
    setupGuide: 'Register an app at portal.azure.com > Azure Active Directory > App registrations',
    docsUrl: 'https://learn.microsoft.com/en-us/graph/overview',
    skillId: 'microsoft365',
  },
];

// ─── Component ───────────────────────────────────────────────────────────────

export function ConnectorsTab() {
  const { config } = useSettings();
  const [credentials, setCredentials] = useState<IntegrationCredentialInfo[]>([]);
  const [k2kCapabilities, setK2kCapabilities] = useState<K2KCapability[]>([]);
  const [setupService, setSetupService] = useState<string | null>(null);
  const [showWizard, setShowWizard] = useState(false);
  const [fieldValues, setFieldValues] = useState<Record<string, string>>({});
  const [selectedScope, setSelectedScope] = useState<string>('readonly');
  const [saving, setSaving] = useState(false);
  const [testing, setTesting] = useState<string | null>(null);
  const [testResult, setTestResult] = useState<{ service: string; ok: boolean; msg: string } | null>(null);

  const loadCredentials = useCallback(async () => {
    try {
      const creds = await invoke<IntegrationCredentialInfo[]>('list_integration_credentials');
      setCredentials(creds);
    } catch (error) {
      notifyError('Connectors', `Failed to load credentials: ${error}`);
    }
  }, []);

  const loadK2kCapabilities = useCallback(async () => {
    if (!config?.k2k?.enabled) return;
    try {
      const caps = await invoke<K2KCapability[]>('list_agent_capabilities');
      setK2kCapabilities(caps.filter(c => c.category === 'Tool'));
    } catch {
      // K2K not available — that's fine
    }
  }, [config?.k2k?.enabled]);

  useEffect(() => {
    loadCredentials();
    loadK2kCapabilities();
  }, [loadCredentials, loadK2kCapabilities]);

  // Group credentials by service
  const credsByService = credentials.reduce<Record<string, IntegrationCredentialInfo[]>>((acc, c) => {
    if (!acc[c.service]) acc[c.service] = [];
    acc[c.service].push(c);
    return acc;
  }, {});

  const connectedServiceIds = Object.keys(credsByService);

  const handleConnect = (serviceId: string) => {
    setSetupService(serviceId);
    setFieldValues({});
    setSelectedScope('readonly');
    setTestResult(null);
  };

  const handleSave = async () => {
    if (!setupService) return;
    const def = SERVICE_DEFINITIONS.find(s => s.id === setupService);
    if (!def) return;

    // Validate required fields
    for (const field of def.fields) {
      if (field.required && !fieldValues[field.key]?.trim()) {
        setTestResult({ service: setupService, ok: false, msg: `${field.label} is required` });
        return;
      }
    }

    setSaving(true);
    try {
      for (const field of def.fields) {
        const value = fieldValues[field.key]?.trim();
        if (value) {
          await invoke('store_integration_credential', {
            service: setupService,
            keyName: field.key,
            value,
            scope: selectedScope,
            label: `${def.name} - ${field.label}`,
          });
        }
      }
      setSetupService(null);
      setFieldValues({});
      await loadCredentials();
    } catch (error) {
      setTestResult({ service: setupService, ok: false, msg: `Failed to save: ${error}` });
    } finally {
      setSaving(false);
    }
  };

  const handleRemove = async (serviceId: string) => {
    const creds = credsByService[serviceId] || [];
    for (const c of creds) {
      try {
        await invoke('delete_integration_credential', { service: c.service, keyName: c.key_name });
      } catch (error) {
        notifyError('Connectors', `Failed to remove credential: ${error}`);
      }
    }
    await loadCredentials();
  };

  const handleTest = async (serviceId: string) => {
    const creds = credsByService[serviceId] || [];
    if (creds.length === 0) return;
    setTesting(serviceId);
    setTestResult(null);
    try {
      const c = creds[0];
      await invoke<string>('test_integration_credential', { service: c.service, keyName: c.key_name });
      setTestResult({ service: serviceId, ok: true, msg: 'Credential verified' });
    } catch (error) {
      setTestResult({ service: serviceId, ok: false, msg: `${error}` });
    } finally {
      setTesting(null);
    }
  };

  const setupDef = setupService ? SERVICE_DEFINITIONS.find(s => s.id === setupService) : null;

  if (!config) return null;

  return (
    <div className="tab-content">
      {/* KN Cloud Connector Wizard */}
      {showWizard && (
        <ConnectorWizard onClose={() => setShowWizard(false)} />
      )}

      {/* KN Cloud Connectors section */}
      <div className="settings-group">
        <h3>
          Knowledge Nexus Connectors
          <InfoTip text="OAuth-based connectors managed by the Knowledge Nexus backend. Authorize once — NexiBot syncs your email, calendar, and files automatically." />
        </h3>
        <p className="group-description">
          Connect Gmail, Google Drive, Google Calendar, Outlook, and more.
          Authorisation happens in your browser — your passwords never touch NexiBot.
        </p>
        <button
          className="save-btn"
          style={{ marginTop: 4 }}
          onClick={() => setShowWizard(true)}
          data-testid="connector-wizard-btn"
        >
          Connect a service
        </button>
      </div>

      {/* Connected Services */}
      {connectedServiceIds.length > 0 && (
        <div className="settings-group">
          <h3>Connected Services<InfoTip text="Services with stored API credentials. Credentials are securely stored in your OS keyring and never exposed to the AI model." /></h3>
          <div className="connector-cards">
            {connectedServiceIds.map(serviceId => {
              const def = SERVICE_DEFINITIONS.find(s => s.id === serviceId);
              const creds = credsByService[serviceId];
              const scope = creds[0]?.scope || 'unknown';
              return (
                <div key={serviceId} className="connector-card connected">
                  <div className="connector-card-header">
                    <span className="status-dot connected" />
                    <span className="connector-name">{def?.name || serviceId}</span>
                    <span className={`scope-badge ${scope}`}>{scope}</span>
                    <span className="key-count">{creds.length} key{creds.length !== 1 ? 's' : ''}</span>
                  </div>
                  <div className="connector-card-actions">
                    <button
                      className="test-button small"
                      onClick={() => handleTest(serviceId)}
                      disabled={testing === serviceId}
                    >
                      {testing === serviceId ? 'Testing...' : 'Test'}
                    </button>
                    <button className="secondary-button small danger" onClick={() => handleRemove(serviceId)}>
                      Remove
                    </button>
                  </div>
                  {testResult && testResult.service === serviceId && (
                    <div className={`test-result ${testResult.ok ? 'success' : 'error'}`}>
                      {testResult.msg}
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        </div>
      )}

      {/* Setup Form (shown when connecting) */}
      {setupDef && (
        <div className="settings-group">
          <h3>Connect {setupDef.name}</h3>
          <p className="group-description">{setupDef.description}</p>

          <div className="setup-guide">
            <strong>Setup:</strong> {setupDef.setupGuide}
            {setupDef.docsUrl && (
              <> &mdash; <a href={setupDef.docsUrl} target="_blank" rel="noopener noreferrer">API Docs</a></>
            )}
          </div>

          {setupDef.fields.map(field => (
            <label key={field.key} className="field">
              <span>{field.label}{field.required && ' *'}</span>
              <input
                type={field.type}
                value={fieldValues[field.key] || ''}
                placeholder={field.placeholder}
                onChange={(e) => setFieldValues({ ...fieldValues, [field.key]: e.target.value })}
              />
            </label>
          ))}

          <label className="field">
            <span>Access Scope<InfoTip text="Controls what the AI can do with this service. Read-only is safer; read-write allows creating and modifying data." /></span>
            <div className="scope-selector">
              {setupDef.scopes.map(scope => (
                <label key={scope.id} className={`scope-option ${selectedScope === scope.id ? 'selected' : ''}`}>
                  <input
                    type="radio"
                    name="scope"
                    value={scope.id}
                    checked={selectedScope === scope.id}
                    onChange={() => setSelectedScope(scope.id)}
                  />
                  <div>
                    <strong>{scope.label}</strong>
                    <span className="scope-desc">{scope.description}</span>
                  </div>
                </label>
              ))}
            </div>
          </label>

          {testResult && testResult.service === setupService && !testResult.ok && (
            <div className="test-result error">{testResult.msg}</div>
          )}

          <div className="setup-actions">
            <button className="save-btn" onClick={handleSave} disabled={saving}>
              {saving ? 'Saving...' : 'Save Credentials'}
            </button>
            <button className="secondary-button" onClick={() => setSetupService(null)}>Cancel</button>
          </div>
        </div>
      )}

      {/* Add Integration */}
      {!setupDef && (
        <div className="settings-group">
          <h3>Add Integration<InfoTip text="Connect external SaaS services. API credentials are stored securely in your OS keyring and injected only when the corresponding integration skill is active." /></h3>
          <p className="group-description">
            Connect external services to enable the AI to search, create, and manage data across your tools.
            Credentials are stored in your OS keyring — the AI never sees raw API keys.
          </p>
          <div className="connector-grid">
            {SERVICE_DEFINITIONS
              .filter(def => !connectedServiceIds.includes(def.id))
              .map(def => (
                <div key={def.id} className="connector-card available" onClick={() => handleConnect(def.id)}>
                  <div className="connector-card-header">
                    <span className="connector-name">{def.name}</span>
                  </div>
                  <p className="connector-desc">{def.description}</p>
                  <button className="secondary-button small">Connect</button>
                </div>
              ))}
          </div>
        </div>
      )}

      {/* K2K Discovered Services */}
      {config.k2k?.enabled && (
        <CollapsibleSection title="K2K Discovered Services">
          <p className="group-description">
            Services discovered from the local System Agent via K2K protocol. These are managed by the System Agent and don't require separate credentials in NexiBot.
          </p>
          {k2kCapabilities.length === 0 ? (
            <div className="no-models">
              <p>No tool capabilities discovered. Make sure the System Agent is running at {config.k2k?.local_agent_url || 'localhost:8765'}.</p>
              <button className="secondary-button" onClick={loadK2kCapabilities}>Refresh</button>
            </div>
          ) : (
            <div className="connector-cards">
              {k2kCapabilities.map(cap => (
                <div key={cap.id} className="connector-card k2k">
                  <div className="connector-card-header">
                    <span className="status-dot k2k" />
                    <span className="connector-name">{cap.name}</span>
                    <span className="scope-badge k2k">{cap.category}</span>
                  </div>
                  <p className="connector-desc">{cap.description}</p>
                  {cap.version && <span className="connector-version">v{cap.version}</span>}
                </div>
              ))}
            </div>
          )}
        </CollapsibleSection>
      )}
    </div>
  );
}
