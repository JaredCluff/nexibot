// =============================================================================
// BackendContext -- React Context providing the BackendAdapter
// =============================================================================
// Wrap your component tree with <BackendProvider> so that any child can call
// `useBackend()` to get the current adapter (Tauri or HTTP).
//
// Usage:
//   <BackendProvider>          {/* auto-detects Tauri vs HTTP */}
//     <App />
//   </BackendProvider>
//
//   <BackendProvider backendUrl="http://192.168.1.50:11434">
//     <App />                  {/* forces HTTP mode to the given URL */}
//   </BackendProvider>
// =============================================================================

import React, { createContext, useMemo } from 'react';
import { BackendAdapter, createBackendAdapter } from './useBackend';

// ---------------------------------------------------------------------------
// Context (exported so useBackend can read it without a circular dep)
// ---------------------------------------------------------------------------

export const BackendContext = createContext<BackendAdapter | null>(null);

// ---------------------------------------------------------------------------
// Provider component
// ---------------------------------------------------------------------------

export interface BackendProviderProps {
  /**
   * Optional explicit backend URL.
   * - When set, forces the HTTP backend regardless of whether Tauri is present.
   * - When omitted, auto-detects: uses TauriBackend if `window.__TAURI_INTERNALS__`
   *   exists, otherwise falls back to HttpBackend with the default URL.
   */
  backendUrl?: string;

  /**
   * Optional pre-built adapter. Takes priority over `backendUrl` and auto-detection.
   * Useful for testing or for providing a custom adapter implementation.
   */
  adapter?: BackendAdapter;

  children: React.ReactNode;
}

/**
 * Provides a `BackendAdapter` to the component tree via React Context.
 *
 * The adapter is created once (memoised on `backendUrl` / `adapter`) and
 * remains stable across re-renders so that downstream hooks and effects
 * don't re-fire unnecessarily.
 */
export function BackendProvider({ backendUrl, adapter, children }: BackendProviderProps) {
  const resolvedAdapter = useMemo<BackendAdapter>(() => {
    // If the caller provided an adapter directly, use it.
    if (adapter) return adapter;

    // Otherwise auto-detect.
    return createBackendAdapter(backendUrl);
  }, [backendUrl, adapter]);

  return (
    <BackendContext.Provider value={resolvedAdapter}>
      {children}
    </BackendContext.Provider>
  );
}
