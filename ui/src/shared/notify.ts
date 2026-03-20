import { emit } from '@tauri-apps/api/event';

type ToastLevel = 'info' | 'warning' | 'error' | 'success';

/**
 * Show a toast notification via the NotificationToast event bus.
 * Safe to call from any component without prop drilling.
 */
export function notify(level: ToastLevel, title: string, message: string): void {
  emit('notify:toast', { level, title, message }).catch(() => {
    // Best effort — if event emission fails, log to console as last resort
    console.error(`[notify] ${title}: ${message}`);
  });
}

export const notifyError = (title: string, message: string) => notify('error', title, message);
export const notifyWarn = (title: string, message: string) => notify('warning', title, message);
export const notifyInfo = (title: string, message: string) => notify('info', title, message);
export const notifySuccess = (title: string, message: string) => notify('success', title, message);
