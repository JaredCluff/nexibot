import { test, expect } from '@playwright/test';
import { injectTauriMock } from '../helpers/tauri-mock';

test.describe('App Launch', () => {
  test.beforeEach(async ({ page }) => {
    await injectTauriMock(page);
  });

  test('renders the main app header', async ({ page }) => {
    await page.goto('/');
    await expect(page.locator('h1')).toContainText('NexiBot');
  });

  test('shows chat interface by default (not onboarding)', async ({ page }) => {
    await page.goto('/');
    // Chat input area should be visible
    await expect(page.locator('textarea, input[type="text"]').first()).toBeVisible({ timeout: 5000 });
  });

  test('header has settings button', async ({ page }) => {
    await page.goto('/');
    await expect(page.locator('.settings-button')).toBeVisible();
  });

  test('header has canvas toggle', async ({ page }) => {
    await page.goto('/');
    await expect(page.locator('.canvas-toggle-button')).toBeVisible();
  });

  test('header has shell viewer button', async ({ page }) => {
    await page.goto('/');
    await expect(page.locator('.shell-viewer-button')).toBeVisible();
  });

  test('sidebar toggle button exists', async ({ page }) => {
    await page.goto('/');
    await expect(page.locator('.sidebar-toggle')).toBeVisible();
  });
});
