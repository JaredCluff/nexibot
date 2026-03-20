/**
 * ConnectorWizard — OAuth-based "Connect Your World" wizard.
 *
 * Integrates with the Knowledge Nexus connector API to let users authorize
 * Gmail, Google Drive, Google Calendar, Outlook/M365, and Notion via server-
 * side OAuth.  Secrets never pass through NexiBot — only a redirect URL is
 * returned, opened in the system browser.  The KN server sends the user back
 * to nexibot://oauth-complete which NexiBot catches via the deep-link plugin
 * and forwards as the "nexibot://deep-link" Tauri event.
 *
 * States:
 *   idle → selecting → authorizing → waiting-for-callback → syncing → complete | error
 */

import { useState, useEffect, useCallback, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen, UnlistenFn } from '@tauri-apps/api/event';
import './ConnectorWizard.css';

// ── Types ──────────────────────────────────────────────────────────────────────

interface ConnectorMeta {
  connector_type: string;
  name: string;
  icon: string;
  category: string;
  capabilities: string[];
  auth_provider: string;
}

interface UserConnector {
  id: string;
  connector_type: string;
  name: string;
  status: string;
  sync_enabled: boolean;
  last_auth_at: string | null;
  last_error: string | null;
}

interface ConnectorSyncStatus {
  id: string;
  connector_type: string;
  status: string;
  items_synced: number;
  last_sync_at: string | null;
  error: string | null;
}

type WizardState =
  | 'idle'
  | 'selecting'
  | 'authorizing'
  | 'waiting-for-callback'
  | 'syncing'
  | 'complete'
  | 'error';

interface Props {
  /** Called when user closes/finishes the wizard. */
  onClose: () => void;
  /** If true renders as an inline section (no modal overlay). */
  inline?: boolean;
}

// ── Fallback connector list (shown when KN API is unreachable) ─────────────────

const FALLBACK_CONNECTORS: ConnectorMeta[] = [
  {
    connector_type: 'gmail',
    name: 'Gmail',
    icon: '✉️',
    category: 'Email',
    capabilities: ['read_email', 'search_email', 'send_email'],
    auth_provider: 'google',
  },
  {
    connector_type: 'google_drive',
    name: 'Google Drive',
    icon: '📁',
    category: 'Files',
    capabilities: ['read_files', 'search_files', 'upload_files'],
    auth_provider: 'google',
  },
  {
    connector_type: 'google_calendar',
    name: 'Google Calendar',
    icon: '📅',
    category: 'Calendar',
    capabilities: ['read_events', 'create_events'],
    auth_provider: 'google',
  },
  {
    connector_type: 'outlook',
    name: 'Outlook / Microsoft 365',
    icon: '📧',
    category: 'Email',
    capabilities: ['read_email', 'search_email', 'send_email'],
    auth_provider: 'microsoft',
  },
  {
    connector_type: 'microsoft_calendar',
    name: 'Microsoft Calendar',
    icon: '🗓️',
    category: 'Calendar',
    capabilities: ['read_events', 'create_events'],
    auth_provider: 'microsoft',
  },
  {
    connector_type: 'notion',
    name: 'Notion',
    icon: '📝',
    category: 'Knowledge',
    capabilities: ['read_pages', 'search_pages', 'create_pages'],
    auth_provider: 'notion',
  },
];

function categoryIcon(category: string): string {
  switch (category) {
    case 'Email': return '✉️';
    case 'Files': return '📁';
    case 'Calendar': return '📅';
    case 'Knowledge': return '📝';
    default: return '🔗';
  }
}

function statusLabel(status: string): string {
  switch (status) {
    case 'active': return 'Active';
    case 'syncing': return 'Syncing…';
    case 'error': return 'Error';
    case 'pending_auth': return 'Needs re-auth';
    default: return status;
  }
}

// ── Main component ─────────────────────────────────────────────────────────────

export function ConnectorWizard({ onClose, inline = false }: Props) {
  const [wizardState, setWizardState] = useState<WizardState>('selecting');
  const [supportedConnectors, setSupportedConnectors] = useState<ConnectorMeta[]>([]);
  const [userConnectors, setUserConnectors] = useState<UserConnector[]>([]);
  const [selectedType, setSelectedType] = useState<string | null>(null);
  const [pendingConnectorId, setPendingConnectorId] = useState<string | null>(null);
  const [syncStatus, setSyncStatus] = useState<ConnectorSyncStatus | null>(null);
  const [errorMsg, setErrorMsg] = useState<string>('');
  const [loadingConnectors, setLoadingConnectors] = useState(true);
  const [deletingId, setDeletingId] = useState<string | null>(null);

  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const deepLinkUnlistenRef = useRef<UnlistenFn | null>(null);

  // ── Load supported + user connectors ──────────────────────────────────────

  const loadSupportedConnectors = useCallback(async () => {
    try {
      const connectors = await invoke<ConnectorMeta[]>('get_supported_connectors');
      setSupportedConnectors(connectors.length > 0 ? connectors : FALLBACK_CONNECTORS);
    } catch {
      setSupportedConnectors(FALLBACK_CONNECTORS);
    }
  }, []);

  const loadUserConnectors = useCallback(async () => {
    try {
      const connectors = await invoke<UserConnector[]>('list_user_connectors');
      setUserConnectors(connectors);
    } catch {
      // Not authenticated — no connectors to show
    }
  }, []);

  useEffect(() => {
    const load = async () => {
      setLoadingConnectors(true);
      await Promise.all([loadSupportedConnectors(), loadUserConnectors()]);
      setLoadingConnectors(false);
    };
    load();
  }, [loadSupportedConnectors, loadUserConnectors]);

  // ── Deep-link listener (nexibot://oauth-complete) ─────────────────────────

  useEffect(() => {
    let unlisten: UnlistenFn | null = null;

    listen<{ url: string }>('nexibot://deep-link', (event) => {
      const { url } = event.payload;
      if (!url.startsWith('nexibot://oauth-complete')) return;

      const parsed = new URL(url);
      const status = parsed.searchParams.get('status');
      const connectorId = parsed.searchParams.get('connector_id');
      const errMsg = parsed.searchParams.get('error');

      if (status === 'ok' && connectorId) {
        setPendingConnectorId(connectorId);
        setWizardState('syncing');
      } else {
        setErrorMsg(errMsg || 'OAuth authorisation failed.');
        setWizardState('error');
      }
    }).then((fn) => {
      unlisten = fn;
      deepLinkUnlistenRef.current = fn;
    });

    return () => {
      unlisten?.();
    };
  }, []);

  // ── Sync status polling ──────────────────────────────────────────────────────

  useEffect(() => {
    if (wizardState !== 'syncing' || !pendingConnectorId) return;

    const poll = async () => {
      try {
        const status = await invoke<ConnectorSyncStatus>('poll_connector_status', {
          connectorId: pendingConnectorId,
        });
        setSyncStatus(status);

        if (status.status === 'active') {
          setWizardState('complete');
          clearInterval(pollRef.current!);
          await loadUserConnectors();
        } else if (status.status === 'error') {
          setErrorMsg(status.error || 'Sync failed.');
          setWizardState('error');
          clearInterval(pollRef.current!);
        }
      } catch (e) {
        // Polling errors are transient — keep trying
      }
    };

    poll();
    pollRef.current = setInterval(poll, 3000);
    return () => clearInterval(pollRef.current!);
  }, [wizardState, pendingConnectorId, loadUserConnectors]);

  // ── OAuth flow start ──────────────────────────────────────────────────────

  const handleAuthorize = useCallback(async (connectorType: string) => {
    setSelectedType(connectorType);
    setErrorMsg('');
    setWizardState('authorizing');

    try {
      // This opens the system browser — Rust does `open::that(&auth_url)`
      await invoke('start_connector_oauth', { connectorType });
      setWizardState('waiting-for-callback');
    } catch (e) {
      setErrorMsg(`Could not start authorisation: ${e}`);
      setWizardState('error');
    }
  }, []);

  // ── Delete connector ──────────────────────────────────────────────────────

  const handleDelete = useCallback(async (connectorId: string) => {
    setDeletingId(connectorId);
    try {
      await invoke('delete_connector', { connectorId });
      await loadUserConnectors();
    } catch (e) {
      setErrorMsg(`Failed to delete connector: ${e}`);
    } finally {
      setDeletingId(null);
    }
  }, [loadUserConnectors]);

  // ── Reset to selection screen ─────────────────────────────────────────────

  const handleReset = useCallback(() => {
    setSelectedType(null);
    setPendingConnectorId(null);
    setSyncStatus(null);
    setErrorMsg('');
    setWizardState('selecting');
    clearInterval(pollRef.current!);
  }, []);

  // ── Helpers ───────────────────────────────────────────────────────────────

  const connectedTypes = new Set(userConnectors.map((c) => c.connector_type));

  const availableConnectors = supportedConnectors.filter(
    (c) => !connectedTypes.has(c.connector_type)
  );

  const selectedMeta = supportedConnectors.find((c) => c.connector_type === selectedType);

  // ── Render ────────────────────────────────────────────────────────────────

  const content = (
    <div className="connector-wizard">
      {/* ── State: selecting ─────────────────────────────────────────────── */}
      {wizardState === 'selecting' && (
        <>
          {/* Connected connectors */}
          {userConnectors.length > 0 && (
            <section className="cw-section">
              <h4 className="cw-section-title">Connected</h4>
              <div className="cw-connected-list">
                {userConnectors.map((uc) => {
                  const meta = supportedConnectors.find((c) => c.connector_type === uc.connector_type);
                  return (
                    <div key={uc.id} className={`cw-connected-card cw-status-${uc.status}`}>
                      <span className="cw-card-icon">{meta?.icon || categoryIcon(meta?.category || '')}</span>
                      <div className="cw-card-info">
                        <span className="cw-card-name">{uc.name}</span>
                        <span className="cw-card-status">{statusLabel(uc.status)}</span>
                        {uc.last_error && <span className="cw-card-error">{uc.last_error}</span>}
                      </div>
                      <button
                        className="cw-remove-btn"
                        onClick={() => handleDelete(uc.id)}
                        disabled={deletingId === uc.id}
                        aria-label={`Remove ${uc.name}`}
                      >
                        {deletingId === uc.id ? '…' : '×'}
                      </button>
                    </div>
                  );
                })}
              </div>
            </section>
          )}

          {/* Available connectors */}
          {loadingConnectors ? (
            <div className="cw-loading">Loading connectors…</div>
          ) : availableConnectors.length === 0 && userConnectors.length > 0 ? (
            <p className="cw-all-connected">All available connectors are connected.</p>
          ) : (
            <section className="cw-section">
              <h4 className="cw-section-title">
                {userConnectors.length === 0 ? 'Connect your world' : 'Add another'}
              </h4>
              <p className="cw-section-desc">
                Securely authorize access via your browser. Credentials are stored server-side — NexiBot never sees your passwords.
              </p>
              <div className="cw-connector-grid">
                {availableConnectors.map((c) => (
                  <button
                    key={c.connector_type}
                    className="cw-connector-card"
                    onClick={() => handleAuthorize(c.connector_type)}
                  >
                    <span className="cw-card-icon">{c.icon || categoryIcon(c.category)}</span>
                    <span className="cw-card-name">{c.name}</span>
                    <span className="cw-card-category">{c.category}</span>
                  </button>
                ))}
              </div>
            </section>
          )}

          <div className="cw-footer">
            <button className="cw-close-btn" onClick={onClose}>
              {userConnectors.length > 0 ? 'Done' : 'Not now'}
            </button>
          </div>
        </>
      )}

      {/* ── State: authorizing ───────────────────────────────────────────── */}
      {wizardState === 'authorizing' && (
        <div className="cw-status-screen">
          <div className="cw-spinner" aria-hidden="true" />
          <p className="cw-status-title">Opening browser…</p>
          <p className="cw-status-desc">Starting secure OAuth for {selectedMeta?.name || selectedType}.</p>
          <button className="cw-link-btn" onClick={handleReset}>Cancel</button>
        </div>
      )}

      {/* ── State: waiting-for-callback ──────────────────────────────────── */}
      {wizardState === 'waiting-for-callback' && (
        <div className="cw-status-screen">
          <div className="cw-spinner cw-spinner-amber" aria-hidden="true" />
          <p className="cw-status-title">Waiting for authorisation…</p>
          <p className="cw-status-desc">
            Complete the sign-in in your browser, then return here.<br />
            NexiBot will detect the callback automatically.
          </p>
          <button className="cw-link-btn" onClick={handleReset}>Cancel</button>
        </div>
      )}

      {/* ── State: syncing ───────────────────────────────────────────────── */}
      {wizardState === 'syncing' && (
        <div className="cw-status-screen">
          <div className="cw-spinner cw-spinner-green" aria-hidden="true" />
          <p className="cw-status-title">Syncing…</p>
          {syncStatus && (
            <p className="cw-status-desc">
              {syncStatus.items_synced > 0
                ? `Synced ${syncStatus.items_synced.toLocaleString()} items`
                : 'Starting sync…'}
            </p>
          )}
          <p className="cw-status-hint">This may take a few minutes for large mailboxes.</p>
        </div>
      )}

      {/* ── State: complete ──────────────────────────────────────────────── */}
      {wizardState === 'complete' && (
        <div className="cw-status-screen cw-success">
          <span className="cw-checkmark" aria-hidden="true">✓</span>
          <p className="cw-status-title">{selectedMeta?.name || 'Connector'} is ready!</p>
          {syncStatus && syncStatus.items_synced > 0 && (
            <p className="cw-status-desc">
              Synced {syncStatus.items_synced.toLocaleString()} items.
            </p>
          )}
          <p className="cw-status-hint">
            Ask me anything about your {selectedMeta?.category?.toLowerCase() || 'data'}.
          </p>
          <div className="cw-complete-actions">
            <button className="cw-primary-btn" onClick={handleReset}>Connect another</button>
            <button className="cw-close-btn" onClick={onClose}>Done</button>
          </div>
        </div>
      )}

      {/* ── State: error ─────────────────────────────────────────────────── */}
      {wizardState === 'error' && (
        <div className="cw-status-screen cw-error">
          <span className="cw-error-icon" aria-hidden="true">✗</span>
          <p className="cw-status-title">Connection failed</p>
          <p className="cw-status-desc">{errorMsg}</p>
          <div className="cw-complete-actions">
            <button className="cw-primary-btn" onClick={handleReset}>Try again</button>
            <button className="cw-close-btn" onClick={onClose}>Cancel</button>
          </div>
        </div>
      )}
    </div>
  );

  if (inline) return content;

  return (
    <div className="cw-modal-overlay" role="dialog" aria-modal="true" aria-label="Connector Wizard">
      <div className="cw-modal">
        <div className="cw-modal-header">
          <h3>Connect Your World</h3>
          <button className="cw-modal-close" onClick={onClose} aria-label="Close wizard">×</button>
        </div>
        {content}
      </div>
    </div>
  );
}
