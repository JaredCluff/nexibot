import { useState, useEffect, useCallback, Component, ErrorInfo, ReactNode } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { listen, UnlistenFn } from '@tauri-apps/api/event';
import Chat from './components/Chat';
import Settings from './components/settings/Settings';
import Onboarding from './components/Onboarding';
import AuthPrompt from './components/AuthPrompt';
import HistorySidebar from './components/HistorySidebar';
import NotificationToast from './components/NotificationToast';
import Canvas, { Artifact } from './components/Canvas';
import ShellViewerApp from './components/ShellViewerApp';
import YoloApprovalBanner from './components/YoloApprovalBanner';
import { notifyError } from './shared/notify';
import './App.css';

class ErrorBoundary extends Component<{ children: ReactNode }, { hasError: boolean; error: string }> {
  constructor(props: { children: ReactNode }) {
    super(props);
    this.state = { hasError: false, error: '' };
  }
  static getDerivedStateFromError(error: Error) {
    return { hasError: true, error: error.message };
  }
  componentDidCatch(_error: Error, _info: ErrorInfo) {
    // React already logs ErrorBoundary errors to the console
  }
  render() {
    if (this.state.hasError) {
      return (
        <div className="error-boundary">
          <h3>Something went wrong</h3>
          <p>Try reloading the app. If the problem persists, check the console for details.</p>
          <pre>{this.state.error}</pre>
          <button
            className="error-boundary-reload"
            onClick={() => window.location.reload()}
          >
            Reload App
          </button>
        </div>
      );
    }
    return this.props.children;
  }
}

function App() {
  const [windowLabel, setWindowLabel] = useState('main');
  const [showSettings, setShowSettings] = useState(false);
  const [showOnboarding, setShowOnboarding] = useState(false);
  const [showAuthPrompt, setShowAuthPrompt] = useState(false);
  const [authPromptReason, setAuthPromptReason] = useState<string | undefined>(undefined);
  const [authPromptProvider, setAuthPromptProvider] = useState<'claude' | 'openai' | undefined>(undefined);
  const [isCheckingFirstRun, setIsCheckingFirstRun] = useState(true);
  const [showSidebar, setShowSidebar] = useState(false);
  const [currentSessionId, setCurrentSessionId] = useState<string | undefined>(undefined);
  const [canvasOpen, setCanvasOpen] = useState(false);
  const [artifacts, setArtifacts] = useState<Artifact[]>([]);
  const [defenseStatus, setDefenseStatus] = useState<string | null>(null);

  useEffect(() => {
    try { setWindowLabel(getCurrentWindow().label); } catch { /* outside Tauri */ }
  }, []);

  useEffect(() => {
    checkFirstRun();
    // Create initial conversation session if needed
    if (!currentSessionId) {
      invoke<string>('new_conversation')
        .then(setCurrentSessionId)
        .catch((e) => notifyError('Session', `Failed to create initial conversation: ${e}`));
    }
  }, []);

  // Listen for canvas:push events from Tauri backend
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;

    listen<Artifact>('canvas:push', (event) => {
      const artifact = event.payload;
      if (!artifact.id) {
        artifact.id = `artifact-${Date.now()}-${Math.random().toString(36).slice(2, 9)}`;
      }
      setArtifacts((prev) => [...prev, artifact]);
      setCanvasOpen(true);
    }).then((fn) => { unlisten = fn; });

    return () => { unlisten?.(); };
  }, []);

  // Listen for canvas:panel-updated events from Tauri backend
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;

    listen<{ panel_id: string; content: string; updated_at: string }>('canvas:panel-updated', (event) => {
      const { panel_id, content } = event.payload;
      setArtifacts((prev) =>
        prev.map((a) => a.id === panel_id ? { ...a, content } : a)
      );
    }).then((fn) => { unlisten = fn; });

    return () => { unlisten?.(); };
  }, []);

  // Listen for defense model loading events
  useEffect(() => {
    const unlistenLoading = listen('defense:loading', () => {
      setDefenseStatus('Loading defense models...');
    });
    const unlistenLoaded = listen('defense:loaded', (event: { payload: { deberta_loaded?: boolean; llama_guard_loaded?: boolean; status?: string } }) => {
      const payload = event.payload;
      if (payload.deberta_loaded || payload.llama_guard_loaded) {
        setDefenseStatus('Defense models ready');
      } else if (payload.status === 'degraded') {
        setDefenseStatus('Defense running in degraded mode');
      } else {
        setDefenseStatus(null);
        return;
      }
      // Auto-hide after 3 seconds
      setTimeout(() => setDefenseStatus(null), 3000);
    });

    return () => {
      unlistenLoading.then(f => f());
      unlistenLoaded.then(f => f());
    };
  }, []);

  const handleOpenInCanvas = useCallback((code: string, language: string) => {
    const artifact: Artifact = {
      id: `artifact-${Date.now()}-${Math.random().toString(36).slice(2, 9)}`,
      type: language === 'html' ? 'html' : language === 'svg' ? 'svg' : language === 'mermaid' ? 'mermaid' : 'code',
      language,
      content: code,
      title: language ? `${language} snippet` : 'Code snippet',
    };
    setArtifacts((prev) => [...prev, artifact]);
    setCanvasOpen(true);
  }, []);

  const handleRemoveArtifact = useCallback((id: string) => {
    setArtifacts((prev) => {
      const remaining = prev.filter((a) => a.id !== id);
      if (remaining.length === 0) {
        setCanvasOpen(false);
      }
      return remaining;
    });
  }, []);

  const checkFirstRun = async () => {
    let window: any;
    try {
      window = getCurrentWindow();
    } catch (err) {
      // Outside Tauri context
      setIsCheckingFirstRun(false);
      return;
    }

    try {
      const isFirst = await invoke<boolean>('is_first_run');

      if (isFirst) {
        setShowOnboarding(true);
        // Show window for onboarding
        await window.show();
        await window.setFocus();
      } else {
        await checkAuthStatus();
        // Show window for normal operation
        await window.show();
        await window.setFocus();
      }
    } catch (_error) {
      // Ensure window is shown even if there's an error
      try {
        await window.show();
        await window.setFocus();
      } catch {
        // Ignore errors
      }
    } finally {
      setIsCheckingFirstRun(false);
    }
  };

  const checkAuthStatus = async () => {
    try {
      const status = await invoke<{ anthropic_configured: boolean; openai_configured: boolean }>('get_provider_status');
      if (!status.anthropic_configured && !status.openai_configured) {
        setAuthPromptReason('No authentication found. Please sign in or enter an API key to use NexiBot.');
        setShowAuthPrompt(true);
      }
    } catch {
      // Auth check failure is non-critical; user can authenticate later
    }
  };

  const handleAuthRequired = (reason?: string) => {
    setAuthPromptReason(reason);
    setAuthPromptProvider(
      reason?.toLowerCase().includes('openai') ? 'openai'
      : reason?.toLowerCase().includes('claude') || reason?.toLowerCase().includes('anthropic') ? 'claude'
      : undefined
    );
    setShowAuthPrompt(true);
  };

  const handleAuthComplete = () => {
    setShowAuthPrompt(false);
    setAuthPromptReason(undefined);
    setAuthPromptProvider(undefined);
  };

  const handleOnboardingComplete = async () => {
    setShowOnboarding(false);

    // Show the main window after onboarding
    const window = getCurrentWindow();
    await window.show();
    await window.setFocus();
  };

  const handleSessionSelect = async (session: { session_id: string }) => {
    try {
      await invoke('load_conversation_session', { sessionId: session.session_id });
      setCurrentSessionId(session.session_id);
      setShowSettings(false);
    } catch (error) {
      notifyError('History', `Failed to load session: ${error}`);
    }
  };

  const handleNewConversation = async () => {
    try {
      const newId = await invoke<string>('new_conversation');
      setCurrentSessionId(newId);
      setShowSettings(false);
    } catch (error) {
      notifyError('Session', `Failed to create new conversation: ${error}`);
    }
  };

  const handleSessionChange = (id: string) => {
    setCurrentSessionId(id);
  };

  if (isCheckingFirstRun) {
    return (
      <div className="app loading">
        <div className="loading-spinner"></div>
      </div>
    );
  }

  if (showOnboarding) {
    return <Onboarding onComplete={handleOnboardingComplete} />;
  }

  if (windowLabel === 'shell-viewer') {
    return <ShellViewerApp />;
  }

  return (
    <div className="app">
      <header className="app-header">
        <button
          className="sidebar-toggle"
          onClick={() => setShowSidebar(!showSidebar)}
          title={showSidebar ? 'Hide history' : 'Show history'}
          aria-label={showSidebar ? 'Hide history' : 'Show history'}
        >
          {showSidebar ? '\u2630' : '\u2630'}
        </button>
        <h1>NexiBot</h1>
        <div className="header-actions">
          <button
            className="canvas-toggle-button"
            onClick={() => setCanvasOpen(!canvasOpen)}
            title={canvasOpen ? 'Hide canvas' : 'Show canvas'}
          >
            {canvasOpen ? 'Hide Canvas' : 'Canvas'}
          </button>
          <button
            className="shell-viewer-button"
            onClick={() => invoke('open_shell_viewer').catch((e) => notifyError('Shell', `Failed to open viewer: ${e}`))}
            title="Open NexiGate Shell Viewer"
          >
            Shell
          </button>
          <button
            className="settings-button"
            onClick={() => setShowSettings(!showSettings)}
            aria-label={showSettings ? 'Show chat' : 'Open settings'}
          >
            {showSettings ? '\uD83D\uDCAC' : '\u2699\uFE0F'}
          </button>
        </div>
      </header>

      {defenseStatus && (
        <div className="defense-status-bar">
          {defenseStatus}
        </div>
      )}

      <YoloApprovalBanner />

      <main className="app-main">
        <HistorySidebar
          isOpen={showSidebar}
          onToggle={() => setShowSidebar(!showSidebar)}
          onSessionSelect={handleSessionSelect}
          onNewConversation={handleNewConversation}
          currentSessionId={currentSessionId}
        />
        <div className={`app-split-layout ${canvasOpen ? 'canvas-visible' : ''}`}>
          <div className="app-content">
            {showSettings ? (
              <ErrorBoundary>
                <Settings onClose={() => setShowSettings(false)} />
              </ErrorBoundary>
            ) : (
              <Chat
                sessionId={currentSessionId}
                onSessionChange={handleSessionChange}
                onAuthRequired={handleAuthRequired}
                onOpenInCanvas={handleOpenInCanvas}
              />
            )}
          </div>
          {canvasOpen && (
            <div className="canvas-panel">
              <Canvas
                artifacts={artifacts}
                onClose={() => setCanvasOpen(false)}
                onRemoveArtifact={handleRemoveArtifact}
              />
            </div>
          )}
        </div>
      </main>
      <NotificationToast />
      {showAuthPrompt && (
        <AuthPrompt
          onComplete={handleAuthComplete}
          onDismiss={() => { setShowAuthPrompt(false); setAuthPromptProvider(undefined); }}
          reason={authPromptReason}
          provider={authPromptProvider}
        />
      )}
    </div>
  );
}

export default App;
