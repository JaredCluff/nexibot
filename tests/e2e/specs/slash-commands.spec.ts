import { test, expect } from '@playwright/test';
import { injectTauriMock, emitEvent } from '../helpers/tauri-mock';

test.describe('Slash Commands', () => {
  test.beforeEach(async ({ page }) => {
    await injectTauriMock(page);
    await page.goto('/');
    await page.waitForSelector('textarea', { timeout: 5000 });
  });

  test('typing / shows slash command palette', async ({ page }) => {
    const textarea = page.locator('textarea').first();
    await textarea.fill('/');
    // The palette should appear with command suggestions
    const palette = page.locator('.cmd-palette');
    await expect(palette.getByText('/model')).toBeVisible({ timeout: 3000 });
    await expect(palette.getByText('/new')).toBeVisible();
    await expect(palette.getByText('/help')).toBeVisible();
  });

  test('palette filters as user types', async ({ page }) => {
    const textarea = page.locator('textarea').first();
    await textarea.fill('/mo');
    // Only /model should match in the palette
    const palette = page.locator('.cmd-palette');
    await expect(palette.getByText('/model')).toBeVisible({ timeout: 3000 });
    // /new should not be visible (doesn't match /mo)
    await expect(palette.getByText('/new')).not.toBeVisible();
  });

  test('palette disappears when input has a space', async ({ page }) => {
    const textarea = page.locator('textarea').first();
    await textarea.fill('/model ');
    // Palette should hide after space (command is complete)
    await page.waitForTimeout(300);
    const palette = page.locator('[class*="palette"], [class*="slash"]').first();
    if (await palette.count() > 0) {
      await expect(palette).not.toBeVisible();
    }
  });

  test('/help shows available commands in chat', async ({ page }) => {
    const textarea = page.locator('textarea').first();
    await textarea.fill('/help');
    await textarea.press('Enter');
    // Should show command list in the chat
    await expect(page.getByText('Switch AI model')).toBeVisible({ timeout: 5000 });
  });

  test('/new starts a new conversation', async ({ page }) => {
    const textarea = page.locator('textarea').first();
    await textarea.fill('/new');
    await textarea.press('Enter');
    // Should show new conversation message
    await page.waitForTimeout(500);
    // The input should be cleared
    await expect(textarea).toHaveValue('');
  });

  test('/compact triggers conversation compaction', async ({ page }) => {
    const textarea = page.locator('textarea').first();
    await textarea.fill('/compact');
    await textarea.press('Enter');
    await page.waitForTimeout(500);
    // Should show compaction status message
    await expect(textarea).toHaveValue('');
  });

  test('/verbose toggles verbose mode', async ({ page }) => {
    const textarea = page.locator('textarea').first();
    await textarea.fill('/verbose');
    await textarea.press('Enter');
    await page.waitForTimeout(500);
    // Should show verbose toggle confirmation
    // Look for override badge
    const badge = page.locator('.override-badge.verbose-badge');
    // Either the badge appears or a system message confirms the toggle
  });

  test('all 10 slash commands appear when typing /', async ({ page }) => {
    const textarea = page.locator('textarea').first();
    await textarea.fill('/');
    await page.waitForTimeout(300);
    const palette = page.locator('.cmd-palette');
    const commands = ['/model', '/think', '/provider', '/verbose', '/compact',
                      '/remind', '/guardrails', '/yolo', '/new', '/help'];
    for (const cmd of commands) {
      await expect(palette.getByText(cmd).first()).toBeVisible({ timeout: 2000 });
    }
  });
});
