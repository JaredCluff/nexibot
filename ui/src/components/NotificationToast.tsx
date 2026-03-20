import { useState, useEffect, useCallback } from 'react';
import { listen, UnlistenFn } from '@tauri-apps/api/event';
import './NotificationToast.css';

interface Toast {
  id: number;
  level: 'info' | 'warning' | 'error' | 'success';
  title: string;
  message: string;
  createdAt: number;
}

interface ToastEventPayload {
  level: string;
  title: string;
  message: string;
}

interface GuiNotificationPayload {
  message: string;
  timestamp?: string;
}

interface ToolBlockedPayload {
  tool_name: string;
  reason: string;
}

interface ToolApprovalExpiredPayload {
  tool_name: string;
}

interface ToolApprovalRequestPayload {
  tool_name: string;
  reason: string;
}

let nextId = 0;

const MAX_VISIBLE = 3;
const DISMISS_MS_DEFAULT = 5000;
const DISMISS_MS_ERROR = 8000;

function inferLevelFromMessage(message: string): Toast['level'] {
  if (message.startsWith('❌')) return 'error';
  if (message.startsWith('⚠')) return 'warning';
  if (message.startsWith('✅')) return 'success';
  return 'info';
}

function NotificationToast() {
  const [toasts, setToasts] = useState<Toast[]>([]);

  const addToast = useCallback((payload: ToastEventPayload) => {
    const level = (['info', 'warning', 'error', 'success'].includes(payload.level)
      ? payload.level
      : 'info') as Toast['level'];

    const toast: Toast = {
      id: nextId++,
      level,
      title: payload.title,
      message: payload.message,
      createdAt: Date.now(),
    };

    setToasts((prev) => {
      const updated = [...prev, toast];
      // Keep only the most recent MAX_VISIBLE
      return updated.slice(-MAX_VISIBLE);
    });

    // Auto-dismiss
    const dismissMs = level === 'error' ? DISMISS_MS_ERROR : DISMISS_MS_DEFAULT;
    setTimeout(() => {
      setToasts((prev) => prev.filter((t) => t.id !== toast.id));
    }, dismissMs);
  }, []);

  const dismissToast = useCallback((id: number) => {
    setToasts((prev) => prev.filter((t) => t.id !== id));
  }, []);

  useEffect(() => {
    const unlisteners: Promise<UnlistenFn>[] = [];

    unlisteners.push(
      listen<ToastEventPayload>('notify:toast', (event) => {
        addToast(event.payload);
      })
    );

    unlisteners.push(
      listen<GuiNotificationPayload>('notification:received', (event) => {
        const message = event.payload?.message;
        if (!message) return;
        addToast({
          level: inferLevelFromMessage(message),
          title: 'Notification',
          message,
        });
      })
    );

    unlisteners.push(
      listen<ToolBlockedPayload>('chat:tool-blocked', (event) => {
        const payload = event.payload;
        if (!payload?.tool_name || !payload?.reason) return;
        addToast({
          level: 'warning',
          title: `Tool Blocked: ${payload.tool_name}`,
          message: payload.reason,
        });
      })
    );

    unlisteners.push(
      listen<ToolApprovalRequestPayload>('chat:tool-approval-request', (event) => {
        const payload = event.payload;
        if (!payload?.tool_name || !payload?.reason) return;
        addToast({
          level: 'warning',
          title: `Approval Required: ${payload.tool_name}`,
          message: payload.reason,
        });
      })
    );

    unlisteners.push(
      listen<ToolApprovalExpiredPayload>('chat:tool-approval-expired', (event) => {
        const payload = event.payload;
        if (!payload?.tool_name) return;
        addToast({
          level: 'warning',
          title: `Approval Timed Out: ${payload.tool_name}`,
          message: 'Tool execution was blocked because no approval was received in time.',
        });
      })
    );

    return () => {
      unlisteners.forEach((p) => p.then((fn) => fn()));
    };
  }, [addToast]);

  if (toasts.length === 0) return null;

  return (
    <div className="toast-container">
      {toasts.map((toast) => (
        <div
          key={toast.id}
          className={`toast toast-${toast.level}`}
          onClick={() => dismissToast(toast.id)}
        >
          <div className="toast-header">
            <span className="toast-title">{toast.title}</span>
            <button className="toast-close" onClick={(e) => { e.stopPropagation(); dismissToast(toast.id); }}>
              &times;
            </button>
          </div>
          <div className="toast-message">{toast.message}</div>
        </div>
      ))}
    </div>
  );
}

export default NotificationToast;
