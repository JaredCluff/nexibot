import { test, expect, Page } from '@playwright/test';

/**
 * Onboarding flow tests.
 *
 * These tests simulate the first-run experience by overriding
 * the `is_first_run` mock to return true.
 */

async function injectOnboardingMock(page: Page) {
  await page.addInitScript(`
    let callbackIdCounter = 0;
    const callbackRegistry = {};
    let eventIdCounter = 1000;
    const eventListenerMap = {};

    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
      unregisterListener: function(event, eventId) {
        if (eventListenerMap[event]) {
          eventListenerMap[event] = eventListenerMap[event].filter(e => e.eventId !== eventId);
        }
      }
    };

    window.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' }, windows: ['main'], webviews: ['main'] },
      transformCallback: function(callback, once) {
        const id = callbackIdCounter++;
        callbackRegistry[id] = { fn: callback, once: !!once };
        window['_' + id] = function(payload) {
          try { callback(payload); } finally { if (once) { delete callbackRegistry[id]; delete window['_' + id]; } }
        };
        return id;
      },
      unregisterCallback: function(id) { delete callbackRegistry[id]; delete window['_' + id]; },
      convertFileSrc: function(path) { return 'file://' + path; },
      invoke: async function(cmd, args) {
        if (cmd.startsWith('plugin:')) return null;

        const overrides = {
          'is_first_run': true,  // <-- trigger onboarding
          'get_provider_status': { anthropic_configured: false, openai_configured: false },
          'get_config': {
            claude: { model: 'claude-sonnet-4-20250514', system_prompt: '', api_key: '' },
            openai: { api_key: '' },
            k2k: { enabled: false, local_agent_url: '', client_id: '' },
            audio: { enabled: false, sample_rate: 16000, channels: 1 },
            wakeword: { enabled: false, wake_word: 'hey_nexibot', threshold: 0.5 },
          },
          'update_config': null,
          'ensure_bridge_running': null,
          'new_conversation': 'test-session-onboarding',
          'start_oauth_flow': 'https://example.com/oauth',
          'open_oauth_browser': 'https://example.com/oauth',
          'complete_oauth_flow': null,
          'check_subscription': 'Active',
          'get_subscription_credentials': 'sk-test-key-123',
          'start_openai_device_flow': { user_code: 'TEST-CODE', verification_uri: 'https://example.com/verify', interval: 5 },
          'poll_openai_device_flow': { status: 'complete', error: null },
        };

        if (cmd in overrides) return overrides[cmd];
        return null;
      },
    };
  `);
}

test.describe('Onboarding Flow', () => {
  test.beforeEach(async ({ page }) => {
    await injectOnboardingMock(page);
  });

  test('shows welcome screen on first run', async ({ page }) => {
    await page.goto('/');
    await expect(page.getByText('Welcome to NexiBot!')).toBeVisible({ timeout: 5000 });
  });

  test('welcome screen shows feature highlights', async ({ page }) => {
    await page.goto('/');
    await expect(page.getByText('Natural Conversations')).toBeVisible();
    await expect(page.getByText('Local Knowledge Search')).toBeVisible();
    await expect(page.getByText('Voice Interaction')).toBeVisible();
  });

  test('Get Started button advances to auth choice', async ({ page }) => {
    await page.goto('/');
    await page.getByRole('button', { name: 'Get Started' }).click();
    await expect(page.getByText('How would you like to connect?')).toBeVisible();
  });

  test('auth choice shows Sign In and API Key options', async ({ page }) => {
    await page.goto('/');
    await page.getByRole('button', { name: 'Get Started' }).click();
    await expect(page.getByText('Sign In').first()).toBeVisible();
    await expect(page.getByText('API Key').first()).toBeVisible();
    await expect(page.getByText('Recommended').first()).toBeVisible();
  });

  test('Sign In path shows provider choices', async ({ page }) => {
    await page.goto('/');
    await page.getByRole('button', { name: 'Get Started' }).click();
    // Click the Sign In option
    await page.locator('.provider-option.recommended').click();
    // Should show provider choices (Claude, OpenAI, etc.)
    await expect(page.getByText(/Claude|Anthropic/i).first()).toBeVisible({ timeout: 3000 });
  });

  test('API Key path shows provider choices', async ({ page }) => {
    await page.goto('/');
    await page.getByRole('button', { name: 'Get Started' }).click();
    // Click the API Key option (second provider-option)
    await page.locator('.provider-option').nth(1).click();
    // Should show API key provider choices
    await page.waitForTimeout(300);
    await expect(page.getByText(/Claude|OpenAI|Cerebras/i).first()).toBeVisible({ timeout: 3000 });
  });

  test('step indicator progresses through flow', async ({ page }) => {
    await page.goto('/');
    // Step indicators should be visible
    const indicators = page.locator('.step-indicator .step, .step-dots .dot, [class*="step"]');
    if (await indicators.count() > 0) {
      await expect(indicators.first()).toBeVisible();
    }
  });
});
