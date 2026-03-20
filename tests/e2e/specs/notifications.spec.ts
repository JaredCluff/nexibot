import { test, expect } from '@playwright/test';
import { injectTauriMock, emitEvent } from '../helpers/tauri-mock';

test.describe('Notification Toasts', () => {
  test.beforeEach(async ({ page }) => {
    await injectTauriMock(page);
    await page.goto('/');
    await page.waitForSelector('textarea', { timeout: 5000 });
  });

  test('info toast appears on notify:toast event', async ({ page }) => {
    await emitEvent(page, 'notify:toast', {
      level: 'info',
      title: 'Test Info',
      message: 'This is an info notification',
    });
    await expect(page.locator('.toast.toast-info')).toBeVisible({ timeout: 3000 });
    await expect(page.getByText('Test Info')).toBeVisible();
    await expect(page.getByText('This is an info notification')).toBeVisible();
  });

  test('error toast appears on notify:toast event', async ({ page }) => {
    await emitEvent(page, 'notify:toast', {
      level: 'error',
      title: 'Test Error',
      message: 'Something went wrong',
    });
    await expect(page.locator('.toast.toast-error')).toBeVisible({ timeout: 3000 });
    await expect(page.getByText('Test Error')).toBeVisible();
  });

  test('warning toast appears on notify:toast event', async ({ page }) => {
    await emitEvent(page, 'notify:toast', {
      level: 'warning',
      title: 'Test Warning',
      message: 'Be careful',
    });
    await expect(page.locator('.toast.toast-warning')).toBeVisible({ timeout: 3000 });
  });

  test('success toast appears on notify:toast event', async ({ page }) => {
    await emitEvent(page, 'notify:toast', {
      level: 'success',
      title: 'Test Success',
      message: 'All good',
    });
    await expect(page.locator('.toast.toast-success')).toBeVisible({ timeout: 3000 });
  });

  test('toast has close button', async ({ page }) => {
    await emitEvent(page, 'notify:toast', {
      level: 'info',
      title: 'Closable',
      message: 'Click to close',
    });
    const toast = page.locator('.toast').first();
    await expect(toast).toBeVisible({ timeout: 3000 });
    const closeBtn = toast.locator('.toast-close');
    await expect(closeBtn).toBeVisible();
  });

  test('clicking close dismisses toast', async ({ page }) => {
    await emitEvent(page, 'notify:toast', {
      level: 'info',
      title: 'Dismiss Me',
      message: 'Will be dismissed',
    });
    const toast = page.locator('.toast').first();
    await expect(toast).toBeVisible({ timeout: 3000 });
    await toast.locator('.toast-close').click();
    await expect(toast).not.toBeVisible({ timeout: 3000 });
  });

  test('tool-blocked event shows warning toast', async ({ page }) => {
    await emitEvent(page, 'chat:tool-blocked', {
      tool_name: 'execute_command',
      reason: 'Command is not on the allowlist',
    });
    await expect(page.locator('.toast.toast-warning')).toBeVisible({ timeout: 3000 });
    await expect(page.getByText('Tool Blocked: execute_command')).toBeVisible();
  });

  test('max 3 toasts visible at once', async ({ page }) => {
    for (let i = 1; i <= 4; i++) {
      await emitEvent(page, 'notify:toast', {
        level: 'info',
        title: `Toast ${i}`,
        message: `Message ${i}`,
      });
    }
    await page.waitForTimeout(500);
    const toasts = page.locator('.toast');
    const count = await toasts.count();
    expect(count).toBeLessThanOrEqual(3);
  });

  test('notification:received event shows toast', async ({ page }) => {
    await emitEvent(page, 'notification:received', {
      message: 'You have a new message from Telegram',
    });
    await expect(page.locator('.toast')).toBeVisible({ timeout: 3000 });
    await expect(page.getByText('You have a new message from Telegram')).toBeVisible();
  });
});
