import { describe, it, expect, beforeEach, vi, afterEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import App from './App';
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';

// Mock Tauri APIs
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: vi.fn(),
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async () => async () => {}),
  once: vi.fn(async () => async () => {}),
  emit: vi.fn(async () => {}),
}));

// Mock child components to simplify testing
vi.mock('./components/Chat', () => ({
  default: () => <div data-testid="chat-component">Chat Component</div>,
}));

vi.mock('./components/Settings', () => ({
  default: () => <div data-testid="settings-component">Settings</div>,
}));

vi.mock('./components/Onboarding', () => ({
  default: ({ onComplete }: any) => (
    <div data-testid="onboarding-component">
      <button onClick={onComplete}>Complete Onboarding</button>
    </div>
  ),
}));

vi.mock('./components/HistorySidebar', () => ({
  default: () => <div data-testid="history-sidebar">Sidebar</div>,
}));

vi.mock('./components/Canvas', () => ({
  default: () => <div data-testid="canvas">Canvas</div>,
}));

vi.mock('./components/NotificationToast', () => ({
  default: () => <div data-testid="toast">Toast</div>,
}));

vi.mock('./components/AuthPrompt', () => ({
  default: () => <div data-testid="auth-prompt">Auth Prompt</div>,
}));

vi.mock('./components/YoloApprovalBanner', () => ({
  default: () => <div data-testid="yolo-banner">Yolo Banner</div>,
}));

describe('App - Window Visibility on Startup', () => {
  let mockWindow: any;
  let mockInvoke: any;

  beforeEach(() => {
    // Reset all mocks before each test
    vi.clearAllMocks();

    // Setup mock window
    mockWindow = {
      show: vi.fn().mockResolvedValue(undefined),
      setFocus: vi.fn().mockResolvedValue(undefined),
      label: 'main',
    };

    mockInvoke = vi.mocked(invoke);
    mockInvoke.mockResolvedValue(undefined); // default fallback for unmocked commands
    vi.mocked(getCurrentWindow).mockReturnValue(mockWindow);
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  it('should show window on normal startup (not first run)', async () => {
    // Setup: not first run, auth configured
    mockInvoke
      .mockResolvedValueOnce(false) // is_first_run: false
      .mockResolvedValueOnce({ anthropic_configured: true, openai_configured: false }); // get_provider_status

    render(<App />);

    // Wait for initial effects to complete
    await waitFor(
      () => {
        expect(mockWindow.show).toHaveBeenCalled();
        expect(mockWindow.setFocus).toHaveBeenCalled();
      },
      { timeout: 2000 }
    );

    // Verify Chat component is rendered
    await waitFor(() => {
      expect(screen.queryByTestId('chat-component')).toBeInTheDocument();
    });
  });

  it('should show window on first run (onboarding path)', async () => {
    // Setup: first run, show onboarding
    mockInvoke.mockResolvedValueOnce(true); // is_first_run: true

    render(<App />);

    // Wait for window to be shown for onboarding
    await waitFor(
      () => {
        expect(mockWindow.show).toHaveBeenCalled();
        expect(mockWindow.setFocus).toHaveBeenCalled();
      },
      { timeout: 2000 }
    );

    // Verify Onboarding component is rendered
    await waitFor(() => {
      expect(screen.queryByTestId('onboarding-component')).toBeInTheDocument();
    });
  });

  it('should show window even if is_first_run command fails', async () => {
    // Setup: invoke fails
    mockInvoke.mockRejectedValueOnce(new Error('Backend error'));

    render(<App />);

    // Window should still be shown via error handler
    await waitFor(
      () => {
        expect(mockWindow.show).toHaveBeenCalled();
        expect(mockWindow.setFocus).toHaveBeenCalled();
      },
      { timeout: 2000 }
    );
  });

  it('should show window even if get_provider_status command fails', async () => {
    // Setup: first command succeeds, second fails
    mockInvoke
      .mockResolvedValueOnce(false) // is_first_run succeeds
      .mockRejectedValueOnce(new Error('Provider status error')); // get_provider_status fails

    render(<App />);

    // Window should still be shown via error handler
    await waitFor(
      () => {
        expect(mockWindow.show).toHaveBeenCalled();
        expect(mockWindow.setFocus).toHaveBeenCalled();
      },
      { timeout: 2000 }
    );
  });

  it('should create initial conversation session on startup', async () => {
    // Setup: not first run
    mockInvoke
      .mockResolvedValueOnce(false) // is_first_run
      .mockResolvedValueOnce({ anthropic_configured: true, openai_configured: false }) // get_provider_status
      .mockResolvedValueOnce('session-123'); // new_conversation

    render(<App />);

    // Wait for new_conversation to be called
    await waitFor(
      () => {
        const newConvCall = mockInvoke.mock.calls.find(
          (call: any[]) => call[0] === 'new_conversation'
        );
        expect(newConvCall).toBeDefined();
      },
      { timeout: 2000 }
    );
  });

  it('should show Chat component when not in onboarding', async () => {
    // Setup: not first run
    mockInvoke
      .mockResolvedValueOnce(false) // is_first_run
      .mockResolvedValueOnce({ anthropic_configured: true, openai_configured: false }); // get_provider_status

    render(<App />);

    // Wait for Chat to appear (not loading spinner, not onboarding)
    await waitFor(
      () => {
        expect(screen.queryByTestId('chat-component')).toBeInTheDocument();
      },
      { timeout: 2000 }
    );
  });

  it('should handle loading state before startup checks complete', async () => {
    // Setup: delay the is_first_run response
    let resolveFirstRun: any;
    mockInvoke.mockReturnValueOnce(
      new Promise((resolve) => {
        resolveFirstRun = resolve;
      })
    );

    render(<App />);

    // Initially should show loading spinner
    expect(document.querySelector('.loading-spinner')).toBeInTheDocument();

    // Resolve the first run check
    resolveFirstRun(false);
    mockInvoke.mockResolvedValueOnce({ anthropic_configured: true, openai_configured: false });

    // Should transition to Chat
    await waitFor(
      () => {
        expect(screen.queryByTestId('chat-component')).toBeInTheDocument();
      },
      { timeout: 2000 }
    );
  });

  it('should not show window if getCurrentWindow throws (outside Tauri)', async () => {
    // Setup: getCurrentWindow throws (not in Tauri context)
    vi.mocked(getCurrentWindow).mockImplementation(() => {
      throw new Error('Not in Tauri context');
    });

    // Should not crash
    render(<App />);

    // Should still render (in browser context)
    await waitFor(() => {
      expect(screen.getByRole('main')).toBeInTheDocument();
    });
  });

  it('should show auth prompt when anthropic not configured', async () => {
    // Setup: not first run, anthropic not configured
    // new_conversation is called in parallel with is_first_run, so it consumes call #2
    mockInvoke
      .mockResolvedValueOnce(false) // is_first_run (call #1)
      .mockResolvedValueOnce('session-123') // new_conversation (call #2, parallel)
      .mockResolvedValueOnce({ anthropic_configured: false, openai_configured: false }); // get_provider_status (call #3)

    render(<App />);

    // Wait for auth prompt to appear
    await waitFor(
      () => {
        expect(screen.queryByTestId('auth-prompt')).toBeInTheDocument();
      },
      { timeout: 2000 }
    );
  });

  it('should not show duplicate window.show() calls', async () => {
    // Setup: not first run
    mockInvoke
      .mockResolvedValueOnce(false) // is_first_run
      .mockResolvedValueOnce({ anthropic_configured: true, openai_configured: false }); // get_provider_status

    render(<App />);

    // Wait for startup to complete
    await waitFor(
      () => {
        expect(mockWindow.show).toHaveBeenCalled();
      },
      { timeout: 2000 }
    );

    // Should be called exactly once during startup
    expect(mockWindow.show).toHaveBeenCalledTimes(1);
  });

  it('should render header with controls after startup', async () => {
    // Setup: not first run
    mockInvoke
      .mockResolvedValueOnce(false) // is_first_run
      .mockResolvedValueOnce({ anthropic_configured: true, openai_configured: false }); // get_provider_status

    render(<App />);

    // Wait for header to appear
    await waitFor(
      () => {
        expect(screen.getByText('NexiBot')).toBeInTheDocument();
      },
      { timeout: 2000 }
    );

    // Verify header controls exist
    expect(screen.getByTitle('Show history')).toBeInTheDocument();
    expect(screen.getByTitle('Open NexiGate Shell Viewer')).toBeInTheDocument();
  });
});
