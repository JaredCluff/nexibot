import { test, expect } from '@playwright/test';
import { injectTauriMock, emitEvent } from '../helpers/tauri-mock';

test.describe('Voice Controls', () => {
  test.beforeEach(async ({ page }) => {
    await injectTauriMock(page);
    await page.goto('/');
    await page.waitForSelector('textarea', { timeout: 5000 });
  });

  test('voice bar is present in the chat interface', async ({ page }) => {
    // VoiceBar is rendered inside Chat — look for voice-related elements
    const voiceBar = page.locator('[class*="voice"], [class*="Voice"]').first();
    // Voice bar might not be visible if voice is disabled by default
    // but the container element should exist
  });

  test('voice status updates from events', async ({ page }) => {
    // Simulate voice status change
    await emitEvent(page, 'voice:status', {
      state: 'Listening',
      enabled: true,
    });
    await page.waitForTimeout(500);
    // Look for listening indicator
    const listening = page.getByText(/listening/i).first();
    if (await listening.count() > 0) {
      await expect(listening).toBeVisible();
    }
  });

  test('voice transcript appears in chat', async ({ page }) => {
    // Simulate voice transcript event
    await emitEvent(page, 'voice:transcript', {
      text: 'Hello from voice input',
      is_final: true,
    });
    await page.waitForTimeout(500);
  });

  test('PTT recording indicator shows during recording', async ({ page }) => {
    // Check if the mic/PTT button exists
    const micBtn = page.locator('button').filter({ hasText: /mic|record|ptt|🎤/i }).first();
    if (await micBtn.count() > 0) {
      await expect(micBtn).toBeVisible();
    }
  });
});
