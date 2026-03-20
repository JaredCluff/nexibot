import { render, screen, act } from '@testing-library/react';
import { beforeEach, afterEach, describe, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import YoloApprovalBanner from './YoloApprovalBanner';

describe('YoloApprovalBanner', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(listen).mockResolvedValue(() => {});
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('hydrates pending request state from get_yolo_status on mount', async () => {
    vi.mocked(invoke).mockResolvedValue({
      active: false,
      approved_at_ms: null,
      expires_at_ms: null,
      remaining_secs: null,
      pending_request: {
        id: 'req-1',
        requested_at_ms: 1710000000000,
        duration_secs: 120,
        reason: 'Need to modify protected config',
      },
    });

    render(<YoloApprovalBanner />);

    expect(await screen.findByText('Yolo Mode Request')).toBeInTheDocument();
    expect(screen.getByText(/Need to modify protected config/i)).toBeInTheDocument();
    expect(vi.mocked(invoke)).toHaveBeenCalledWith('get_yolo_status');
  });

  it('syncs out-of-band approval changes via polling', async () => {
    vi.useFakeTimers();

    vi.mocked(invoke)
      .mockResolvedValueOnce({
        active: false,
        approved_at_ms: null,
        expires_at_ms: null,
        remaining_secs: null,
        pending_request: {
          id: 'req-2',
          requested_at_ms: 1710000000000,
          duration_secs: 30,
          reason: null,
        },
      })
      .mockResolvedValue({
        active: true,
        approved_at_ms: 1710000005000,
        expires_at_ms: 1710000035000,
        remaining_secs: 30,
        pending_request: null,
      });

    render(<YoloApprovalBanner />);
    await act(async () => {
      await Promise.resolve();
    });
    expect(screen.getByText('Yolo Mode Request')).toBeInTheDocument();

    await act(async () => {
      vi.advanceTimersByTime(4100);
      await Promise.resolve();
    });

    expect(screen.getByText(/Yolo mode 30s remaining/i)).toBeInTheDocument();
  });
});
