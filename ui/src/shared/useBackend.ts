// =============================================================================
// Backend Abstraction Layer for NexiBot
// =============================================================================
// Provides a unified interface for communicating with the backend, whether
// running inside Tauri (desktop) or via HTTP/WebSocket (future mobile).
//
// Usage:
//   const backend = useBackend();
//   const config = await backend.invoke<NexiBotConfig>('get_config');
//   const unlisten = await backend.listen<TextChunkPayload>('chat:text-chunk', (payload) => { ... });
// =============================================================================

import { useContext } from 'react';
import { BackendContext } from './BackendContext';

// ---------------------------------------------------------------------------
// BackendAdapter interface
// ---------------------------------------------------------------------------

/**
 * Abstraction over the communication layer to the NexiBot backend.
 *
 * - `invoke` sends a command and waits for a typed response.
 * - `listen` subscribes to a named event stream and returns an unlisten function.
 */
export interface BackendAdapter {
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
  listen<T>(event: string, callback: (payload: T) => void): Promise<() => void>;
}

// ---------------------------------------------------------------------------
// TauriBackend -- wraps @tauri-apps/api for desktop
// ---------------------------------------------------------------------------

export class TauriBackend implements BackendAdapter {
  async invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
    // Dynamic import so the module is only loaded when Tauri is present.
    const { invoke } = await import('@tauri-apps/api/core');
    return invoke<T>(command, args);
  }

  async listen<T>(event: string, callback: (payload: T) => void): Promise<() => void> {
    const { listen } = await import('@tauri-apps/api/event');
    // Tauri's listen wraps the payload inside `event.payload`.
    const unlisten = await listen<T>(event, (tauriEvent) => {
      callback(tauriEvent.payload);
    });
    return unlisten;
  }
}

// ---------------------------------------------------------------------------
// HttpBackend -- wraps fetch() + WebSocket for future mobile / web use
// ---------------------------------------------------------------------------

export class HttpBackend implements BackendAdapter {
  private baseUrl: string;
  private ws: WebSocket | null = null;
  private listenerMap: Map<string, Set<(payload: unknown) => void>> = new Map();
  private wsReady: Promise<void> | null = null;
  private nextReconnectMs = 1000;
  private readonly maxReconnectMs = 30000;

  constructor(baseUrl: string = 'http://localhost:11434') {
    // Strip trailing slash for consistency.
    this.baseUrl = baseUrl.replace(/\/+$/, '');
  }

  // -- invoke via POST -------------------------------------------------

  async invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
    const url = `${this.baseUrl}/api/invoke/${command}`;
    const response = await fetch(url, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: args ? JSON.stringify(args) : '{}',
    });

    if (!response.ok) {
      const text = await response.text();
      throw new Error(`Backend invoke "${command}" failed (${response.status}): ${text}`);
    }

    // If the response body is empty, return undefined cast as T.
    const text = await response.text();
    if (!text) return undefined as unknown as T;

    return JSON.parse(text) as T;
  }

  // -- listen via WebSocket --------------------------------------------

  async listen<T>(event: string, callback: (payload: T) => void): Promise<() => void> {
    this.ensureWebSocket();
    await this.wsReady;

    if (!this.listenerMap.has(event)) {
      this.listenerMap.set(event, new Set());
    }
    const typedCb = callback as (payload: unknown) => void;
    this.listenerMap.get(event)!.add(typedCb);

    // Send a subscribe message so the server knows we care about this event.
    this.wsSend({ type: 'subscribe', event });

    // Return an unlisten function.
    return () => {
      const listeners = this.listenerMap.get(event);
      if (listeners) {
        listeners.delete(typedCb);
        if (listeners.size === 0) {
          this.listenerMap.delete(event);
          this.wsSend({ type: 'unsubscribe', event });
        }
      }
    };
  }

  // -- internal WebSocket management -----------------------------------

  private ensureWebSocket(): void {
    if (this.ws && (this.ws.readyState === WebSocket.OPEN || this.ws.readyState === WebSocket.CONNECTING)) {
      return;
    }

    const wsUrl = this.baseUrl.replace(/^http/, 'ws') + '/api/events';
    this.ws = new WebSocket(wsUrl);

    this.wsReady = new Promise<void>((resolve, reject) => {
      const ws = this.ws!;

      ws.onopen = () => {
        this.nextReconnectMs = 1000;

        // Re-subscribe to all active events.
        for (const event of this.listenerMap.keys()) {
          this.wsSend({ type: 'subscribe', event });
        }

        resolve();
      };

      ws.onerror = (err) => {
        console.error('[HttpBackend] WebSocket error:', err);
        reject(err);
      };

      ws.onmessage = (msgEvent) => {
        try {
          const data = JSON.parse(msgEvent.data as string) as { event: string; payload: unknown };
          const listeners = this.listenerMap.get(data.event);
          if (listeners) {
            for (const cb of listeners) {
              try {
                cb(data.payload);
              } catch (cbErr) {
                console.error(`[HttpBackend] Listener error for "${data.event}":`, cbErr);
              }
            }
          }
        } catch {
          // Non-JSON messages are silently ignored.
        }
      };

      ws.onclose = () => {
        // Reconnect with exponential backoff if there are active listeners.
        if (this.listenerMap.size > 0) {
          setTimeout(() => {
            this.ws = null;
            this.ensureWebSocket();
          }, this.nextReconnectMs);
          this.nextReconnectMs = Math.min(this.nextReconnectMs * 2, this.maxReconnectMs);
        }
      };
    });
  }

  private wsSend(msg: Record<string, unknown>): void {
    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(msg));
    }
  }
}

// ---------------------------------------------------------------------------
// Auto-detection helper
// ---------------------------------------------------------------------------

/**
 * Detect whether we are running inside a Tauri webview.
 * Tauri injects `window.__TAURI_INTERNALS__` into the page.
 */
export function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

/**
 * Create the appropriate backend adapter based on the runtime environment.
 *
 * @param backendUrl - Optional URL for the HTTP backend (ignored when Tauri is detected).
 */
export function createBackendAdapter(backendUrl?: string): BackendAdapter {
  if (isTauri()) {
    return new TauriBackend();
  }
  return new HttpBackend(backendUrl);
}

// ---------------------------------------------------------------------------
// React hook
// ---------------------------------------------------------------------------

/**
 * React hook that returns the current `BackendAdapter` from context.
 *
 * Must be used inside a `<BackendProvider>`.
 *
 * @example
 * ```tsx
 * function MyComponent() {
 *   const backend = useBackend();
 *   useEffect(() => {
 *     backend.invoke<NexiBotConfig>('get_config').then(setConfig);
 *   }, [backend]);
 * }
 * ```
 */
export function useBackend(): BackendAdapter {
  const adapter = useContext(BackendContext);
  if (!adapter) {
    throw new Error(
      'useBackend() must be used inside a <BackendProvider>. ' +
      'Wrap your application (or the component tree) with <BackendProvider>.'
    );
  }
  return adapter;
}
