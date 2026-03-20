/**
 * Tauri IPC Mock for Playwright E2E tests.
 *
 * Injects a fake `window.__TAURI_INTERNALS__` and event plugin so the
 * React app thinks it's running inside Tauri v2. All invoke() calls
 * are intercepted — plugin commands return no-ops, app commands either
 * hit overrides or route to the NexiBot HTTP API server.
 *
 * Usage in a Playwright test:
 *   await injectTauriMock(page);
 *   await page.goto('/');
 */

import { Page } from '@playwright/test';

const API_BASE = process.env.NEXIBOT_API_URL || 'http://127.0.0.1:11434';
const AUTH_TOKEN = process.env.NEXIBOT_AUTH_TOKEN || 'test-token';

/**
 * Inject the Tauri IPC mock into the page before any app code runs.
 * Must be called BEFORE page.goto().
 */
export async function injectTauriMock(
  page: Page,
  apiBase: string = API_BASE,
  commandOverrides: Record<string, unknown> = {},
) {
  const overridesJson = JSON.stringify(commandOverrides);
  await page.addInitScript(`
    // ═══════════════════════════════════════════════════════════════════
    // Tauri v2 IPC Mock for NexiBot E2E Testing
    // ═══════════════════════════════════════════════════════════════════

    const API_BASE = "${apiBase}";
    const AUTH_TOKEN = "${AUTH_TOKEN}";

    // ── Callback registry (used by transformCallback / event system) ──
    let callbackIdCounter = 0;
    const callbackRegistry = {};  // id -> { fn, once }

    // ── Event listener registry ──────────────────────────────────────
    // Maps event name -> Set of callback IDs
    let eventIdCounter = 1000;
    const eventListenerMap = {};  // eventName -> [{ eventId, callbackId }]

    // ── __TAURI_EVENT_PLUGIN_INTERNALS__ ─────────────────────────────
    // Required by @tauri-apps/api/event for _unlisten
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
      unregisterListener: function(event, eventId) {
        if (eventListenerMap[event]) {
          eventListenerMap[event] = eventListenerMap[event]
            .filter(entry => entry.eventId !== eventId);
        }
      }
    };

    // ── __TAURI_INTERNALS__ ──────────────────────────────────────────
    window.__TAURI_INTERNALS__ = {
      metadata: {
        currentWindow: { label: 'main' },
        currentWebview: { label: 'main' },
        windows: ['main'],
        webviews: ['main'],
      },

      // transformCallback: Tauri uses this to register JS callbacks
      // that the backend can invoke by ID. Returns a numeric ID.
      transformCallback: function(callback, once) {
        const id = callbackIdCounter++;
        callbackRegistry[id] = { fn: callback, once: !!once };
        // Tauri expects callbacks to be callable via window['_' + id]
        window['_' + id] = function(payload) {
          try {
            callback(payload);
          } finally {
            if (once) {
              delete callbackRegistry[id];
              delete window['_' + id];
            }
          }
        };
        return id;
      },

      unregisterCallback: function(id) {
        delete callbackRegistry[id];
        delete window['_' + id];
      },

      convertFileSrc: function(path, protocol) {
        return 'file://' + path;
      },

      // ── Core invoke mock ──────────────────────────────────────────
      invoke: async function(cmd, args, options) {
        // --- Plugin: event system ---
        if (cmd === 'plugin:event|listen') {
          const evtName = args?.event;
          const handlerId = args?.handler;
          const eid = eventIdCounter++;
          if (!eventListenerMap[evtName]) eventListenerMap[evtName] = [];
          eventListenerMap[evtName].push({ eventId: eid, callbackId: handlerId });
          return eid;
        }
        if (cmd === 'plugin:event|unlisten') {
          const evtName = args?.event;
          const eid = args?.eventId;
          if (eventListenerMap[evtName]) {
            eventListenerMap[evtName] = eventListenerMap[evtName]
              .filter(e => e.eventId !== eid);
          }
          return null;
        }
        if (cmd === 'plugin:event|emit' || cmd === 'plugin:event|emit_to') {
          // Deliver to local listeners
          fireEvent(args?.event, args?.payload);
          return null;
        }

        // --- Plugin: all other plugin commands (window, dialog, etc.) ---
        if (cmd.startsWith('plugin:')) {
          return null;
        }

        // --- Test-supplied overrides (highest priority) ---
        if (cmd in testOverrides) {
          return testOverrides[cmd];
        }

        // --- App command overrides (fast, no network) ---
        const overrides = {
          'is_first_run': false,
          'get_provider_status': { anthropic_configured: true, openai_configured: false },
          'new_conversation': 'test-session-' + Date.now(),
        };
        if (cmd in overrides) {
          return overrides[cmd];
        }

        // --- Route to HTTP API ---
        try {
          const response = await fetch(API_BASE + '/api/invoke/' + cmd, {
            method: 'POST',
            headers: {
              'Content-Type': 'application/json',
              'Authorization': 'Bearer ' + AUTH_TOKEN,
            },
            body: JSON.stringify(args || {}),
          });

          if (!response.ok) {
            const text = await response.text();
            console.warn('[TAURI_MOCK] invoke HTTP fail:', cmd, response.status, text);
            return getDefaultResponse(cmd);
          }

          const text = await response.text();
          if (!text) return undefined;
          return JSON.parse(text);
        } catch (err) {
          console.warn('[TAURI_MOCK] invoke error:', cmd, err.message);
          return getDefaultResponse(cmd);
        }
      },
    };

    // ── Event firing (for __TEST_EMIT_EVENT__) ──────────────────────
    function fireEvent(eventName, payload) {
      const listeners = eventListenerMap[eventName] || [];
      for (const { callbackId } of listeners) {
        const cbFn = window['_' + callbackId];
        if (cbFn) {
          try {
            // Tauri wraps payload in { event, id, payload }
            cbFn({ event: eventName, id: 0, payload: payload });
          } catch(e) {
            console.error('[TAURI_MOCK] Event handler error:', eventName, e);
          }
        }
      }
    }

    // Expose for test use
    window.__TEST_EMIT_EVENT__ = fireEvent;

    // ── Test-supplied command overrides ──────────────────────────────
    const testOverrides = ${overridesJson};

    // ── Default responses for common commands ────────────────────────
    function getDefaultResponse(cmd) {
      if (cmd in testOverrides) return testOverrides[cmd];
      const defaults = {
        'get_config': {
          claude: { model: 'claude-sonnet-4-20250514', system_prompt: 'You are NexiBot.' },
          webhooks: {},
          telegram: { enabled: false },
          whatsapp: { enabled: false },
          voice: { enabled: false },
          guardrails: { security_level: 'standard' },
        },
        'list_sessions': [],
        'list_conversation_sessions': [],
        'get_guardrails_config': { security_level: 'standard' },
        'list_skills': [],
        'list_agents': [],
        'get_defense_status': { enabled: false, models_loaded: [] },
        'get_voice_status': { state: 'idle', enabled: false },
        'list_mcp_servers': [],
        'get_model_registry': { models: [] },
        'get_session_overrides': {},
        'get_bridge_health': { status: 'ok' },
        'list_scheduled_tasks': [],
        'get_observability_metrics': {},
        'get_yolo_status': { enabled: false, mode: 'off' },
        'get_dashboard_data': { sessions: 0, messages: 0 },
        'compact_conversation': null,
        'load_conversation_session': null,
      };
      return defaults[cmd] ?? null;
    }
  `);
}

/**
 * Simulate a Tauri backend event from within Playwright.
 * This fires the event through the same path as real Tauri events.
 */
export async function emitEvent(page: Page, event: string, payload: unknown) {
  await page.evaluate(
    ({ event, payload }) => {
      (window as any).__TEST_EMIT_EVENT__(event, payload);
    },
    { event, payload }
  );
}
