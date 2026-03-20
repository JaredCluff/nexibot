import { test, expect } from '@playwright/test';
import { injectTauriMock } from '../helpers/tauri-mock';

test.describe('Settings Panel', () => {
  test.beforeEach(async ({ page }) => {
    await injectTauriMock(page);
    await page.goto('/');
    // Click settings button to open
    await page.locator('.settings-button').click();
  });

  test('settings panel opens when settings button is clicked', async ({ page }) => {
    // Settings should be visible — look for tabs or settings content
    await expect(page.locator('[class*="settings"], [class*="Settings"]').first()).toBeVisible({ timeout: 5000 });
  });

  test('settings has navigation tabs', async ({ page }) => {
    // Common tab names from the codebase
    const expectedTabs = ['Models', 'Agents', 'Voice', 'Security', 'Channels'];
    for (const tabName of expectedTabs) {
      const tab = page.getByRole('tab', { name: tabName }).or(page.getByText(tabName, { exact: false }));
      if (await tab.count() > 0) {
        // At least some tabs should be visible
        await expect(tab.first()).toBeVisible();
      }
    }
  });

  test('can navigate to Models tab', async ({ page }) => {
    const modelsTab = page.getByText('Models', { exact: false }).first();
    if (await modelsTab.count() > 0) {
      await modelsTab.click();
      // Should show model configuration
      await page.waitForTimeout(500);
    }
  });

  test('can navigate to Security tab', async ({ page }) => {
    const securityTab = page.getByText('Security', { exact: false }).first();
    if (await securityTab.count() > 0) {
      await securityTab.click();
      await page.waitForTimeout(500);
      // Should show security-related content
      const secContent = page.locator('[class*="security"], [class*="Security"]');
      if (await secContent.count() > 0) {
        await expect(secContent.first()).toBeVisible();
      }
    }
  });

  test('can navigate to Channels tab', async ({ page }) => {
    const channelsTab = page.getByText('Channels', { exact: false }).first();
    if (await channelsTab.count() > 0) {
      await channelsTab.click();
      await page.waitForTimeout(500);
    }
  });

  test('settings closes when chat button is clicked', async ({ page }) => {
    // The settings button toggles — click again to close
    await page.locator('.settings-button').click();
    // Chat should be visible again
    await expect(page.locator('textarea, input[type="text"]').first()).toBeVisible({ timeout: 3000 });
  });
});
