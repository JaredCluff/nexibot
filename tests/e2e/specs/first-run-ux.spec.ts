import { test, expect } from '@playwright/test';
import { injectTauriMock, emitEvent } from '../helpers/tauri-mock';

test.describe('First-Run UX', () => {
  test.beforeEach(async ({ page }) => {
    await injectTauriMock(page, undefined, {
      is_first_run: false,
      get_provider_status: { anthropic_configured: true, openai_configured: false },
      get_defense_status: {
        enabled: true,
        deberta_loaded: false,
        llama_guard_loaded: false,
        fail_open: true,
      },
    });
    await page.goto('/');
    await page.waitForSelector('textarea', { timeout: 5000 });
  });

  test('shows defense loading status when models are loading', async ({ page }) => {
    // Simulate defense:loading event
    await emitEvent(page, 'defense:loading', {
      status: 'loading',
      message: 'Loading defense models...',
    });

    // Should show a loading indicator
    await expect(page.getByText(/loading defense/i)).toBeVisible();
  });

  test('hides defense status after models are loaded', async ({ page }) => {
    // Simulate loading then loaded
    await emitEvent(page, 'defense:loading', { status: 'loading' });
    await emitEvent(page, 'defense:loaded', {
      status: 'ready',
      deberta_loaded: true,
      llama_guard_loaded: false,
    });

    // Status should show "Defense models ready" briefly
    await expect(page.getByText(/defense models ready/i)).toBeVisible();
  });

  test('shows degraded mode when no models loaded but fail_open=true', async ({ page }) => {
    await emitEvent(page, 'defense:loaded', {
      status: 'degraded',
      deberta_loaded: false,
      llama_guard_loaded: false,
    });

    await expect(page.getByText(/degraded/i)).toBeVisible();
  });

  test('chat input is available immediately (not blocked by defense loading)', async ({ page }) => {
    // Even while defense is loading, user should be able to type
    const input = page.locator('textarea').first();
    await expect(input).toBeVisible();
    await input.fill('hello');
    await expect(input).toHaveValue('hello');
  });

  test('defense fail_open=true allows messages when no models loaded', async ({ page }) => {
    // This verifies the UI doesn't show a blocking error
    // The actual defense check happens in backend, but UI should not block
    const input = page.locator('textarea').first();
    await input.fill('hello');

    // Send button should be enabled
    const sendButton = page.getByRole('button', { name: /send/i });
    await expect(sendButton).toBeEnabled();
  });
});
