import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import { renderHook } from '@testing-library/react';
import { BackendProvider } from './BackendContext';
import { useBackend, BackendAdapter } from './useBackend';

// Mock createBackendAdapter so it does not try to detect Tauri or create WebSockets
vi.mock('./useBackend', async () => {
  const actual = await vi.importActual<typeof import('./useBackend')>('./useBackend');
  return {
    ...actual,
    createBackendAdapter: vi.fn(() => ({
      invoke: vi.fn(),
      listen: vi.fn(),
    })),
  };
});

describe('BackendContext', () => {
  it('useBackend throws error without provider', () => {
    // renderHook without a wrapper should trigger the error
    expect(() => {
      renderHook(() => useBackend());
    }).toThrow('useBackend() must be used inside a <BackendProvider>');
  });

  it('BackendProvider provides adapter to children', () => {
    function Consumer() {
      const backend = useBackend();
      return <div data-testid="has-backend">{backend ? 'yes' : 'no'}</div>;
    }

    render(
      <BackendProvider>
        <Consumer />
      </BackendProvider>
    );

    expect(screen.getByTestId('has-backend')).toHaveTextContent('yes');
  });

  it('custom adapter is used when provided', () => {
    const mockAdapter: BackendAdapter = {
      invoke: vi.fn().mockResolvedValue('mock-result'),
      listen: vi.fn().mockResolvedValue(() => {}),
    };

    let capturedAdapter: BackendAdapter | null = null;

    function Consumer() {
      capturedAdapter = useBackend();
      return <div>consumer</div>;
    }

    render(
      <BackendProvider adapter={mockAdapter}>
        <Consumer />
      </BackendProvider>
    );

    expect(capturedAdapter).toBe(mockAdapter);
  });
});
