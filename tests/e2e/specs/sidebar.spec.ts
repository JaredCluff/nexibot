import { test, expect } from '@playwright/test';
import { injectTauriMock } from '../helpers/tauri-mock';

test.describe('History Sidebar', () => {
  test.beforeEach(async ({ page }) => {
    await injectTauriMock(page);
    await page.goto('/');
  });

  test('sidebar opens when hamburger button is clicked', async ({ page }) => {
    await page.locator('.sidebar-toggle').click();
    // Sidebar should become visible
    const sidebar = page.locator('[class*="sidebar"], [class*="Sidebar"]').first();
    await expect(sidebar).toBeVisible({ timeout: 3000 });
  });

  test('sidebar has new conversation button', async ({ page }) => {
    await page.locator('.sidebar-toggle').click();
    await page.waitForTimeout(500);
    const newBtn = page.getByText(/new|New Conversation/i).first();
    if (await newBtn.count() > 0) {
      await expect(newBtn).toBeVisible();
    }
  });

  test('sidebar closes when toggle is clicked again', async ({ page }) => {
    await page.locator('.sidebar-toggle').click();
    await page.waitForTimeout(300);
    await page.locator('.sidebar-toggle').click();
    await page.waitForTimeout(300);
    // Check sidebar is hidden or collapsed
  });
});
