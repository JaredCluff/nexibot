import { render, screen, act } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { listen } from '@tauri-apps/api/event';
import NotificationToast from './NotificationToast';

type Handler = (event: { payload: any }) => void;

let handlers: Record<string, Handler> = {};

function dispatchTauriEvent(event: string, payload: any) {
  const handler = handlers[event];
  expect(handler).toBeTypeOf('function');
  act(() => {
    handler({ payload });
  });
}

describe('NotificationToast', () => {
  beforeEach(() => {
    handlers = {};
    vi.clearAllMocks();
    vi.mocked(listen).mockImplementation(async (event: string, handler: any) => {
      handlers[event] = handler;
      return () => {
        delete handlers[event];
      };
    });
  });

  it('renders toasts from notify:toast events', async () => {
    render(<NotificationToast />);

    dispatchTauriEvent('notify:toast', {
      level: 'success',
      title: 'Saved',
      message: 'Settings updated',
    });

    expect(await screen.findByText('Saved')).toBeInTheDocument();
    expect(screen.getByText('Settings updated')).toBeInTheDocument();
  });

  it('renders toasts from notification:received events', async () => {
    render(<NotificationToast />);

    dispatchTauriEvent('notification:received', {
      message: '✅ Task complete: report generated',
      timestamp: '2026-03-02T12:00:00Z',
    });

    expect(await screen.findByText('Notification')).toBeInTheDocument();
    const message = screen.getByText('✅ Task complete: report generated');
    expect(message).toBeInTheDocument();
    expect(message.closest('.toast')).toHaveClass('toast-success');
  });

  it('renders warning toast from chat:tool-blocked events', async () => {
    render(<NotificationToast />);

    dispatchTauriEvent('chat:tool-blocked', {
      tool_name: 'nexibot_execute',
      reason: 'Tool requires user confirmation',
    });

    expect(await screen.findByText('Tool Blocked: nexibot_execute')).toBeInTheDocument();
    const message = screen.getByText('Tool requires user confirmation');
    expect(message).toBeInTheDocument();
    expect(message.closest('.toast')).toHaveClass('toast-warning');
  });

  it('renders warning toast from chat:tool-approval-request events', async () => {
    render(<NotificationToast />);

    dispatchTauriEvent('chat:tool-approval-request', {
      request_id: 'req-1',
      tool_name: 'browser_click',
      reason: 'External action requires confirmation',
    });

    expect(await screen.findByText('Approval Required: browser_click')).toBeInTheDocument();
    const message = screen.getByText('External action requires confirmation');
    expect(message).toBeInTheDocument();
    expect(message.closest('.toast')).toHaveClass('toast-warning');
  });

  it('renders warning toast from chat:tool-approval-expired events', async () => {
    render(<NotificationToast />);

    dispatchTauriEvent('chat:tool-approval-expired', {
      request_id: 'req-1',
      tool_name: 'browser_click',
    });

    expect(await screen.findByText('Approval Timed Out: browser_click')).toBeInTheDocument();
    const message = screen.getByText(
      'Tool execution was blocked because no approval was received in time.'
    );
    expect(message).toBeInTheDocument();
    expect(message.closest('.toast')).toHaveClass('toast-warning');
  });
});
