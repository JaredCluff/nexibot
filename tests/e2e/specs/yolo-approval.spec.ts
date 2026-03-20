import { test, expect } from '@playwright/test';
import { injectTauriMock, emitEvent } from '../helpers/tauri-mock';

test.describe('Yolo Approval Banner', () => {
  test.beforeEach(async ({ page }) => {
    await injectTauriMock(page);
    await page.goto('/');
    await page.waitForSelector('textarea', { timeout: 5000 });
  });

  test('yolo request pending event shows approval card', async ({ page }) => {
    await emitEvent(page, 'yolo:request-pending', {
      id: 'yolo-req-1',
      requested_at_ms: Date.now(),
      duration_secs: 300,
      reason: 'Need to modify config files',
    });
    await expect(page.locator('.yolo-approval-card')).toBeVisible({ timeout: 3000 });
    await expect(page.getByText('Yolo Mode Request')).toBeVisible();
  });

  test('approval card shows approve and deny buttons', async ({ page }) => {
    await emitEvent(page, 'yolo:request-pending', {
      id: 'yolo-req-2',
      requested_at_ms: Date.now(),
      duration_secs: 600,
      reason: 'Install npm packages',
    });
    await expect(page.locator('.yolo-approve-btn')).toBeVisible({ timeout: 3000 });
    await expect(page.locator('.yolo-deny-btn')).toBeVisible();
  });

  test('approval card shows reason', async ({ page }) => {
    await emitEvent(page, 'yolo:request-pending', {
      id: 'yolo-req-3',
      requested_at_ms: Date.now(),
      duration_secs: 120,
      reason: 'Run git commands',
    });
    await expect(page.getByText('Run git commands')).toBeVisible({ timeout: 3000 });
  });

  test('approval card shows warning about elevated access', async ({ page }) => {
    await emitEvent(page, 'yolo:request-pending', {
      id: 'yolo-req-4',
      requested_at_ms: Date.now(),
      duration_secs: null,
      reason: null,
    });
    await expect(page.getByText(/modify config files|privileged/i)).toBeVisible({ timeout: 3000 });
  });

  test('yolo approved event shows active banner', async ({ page }) => {
    await emitEvent(page, 'yolo:approved', {
      active: true,
      approved_at_ms: Date.now(),
      expires_at_ms: Date.now() + 300000,
      remaining_secs: 300,
      pending_request: null,
    });
    await expect(page.locator('.yolo-active-banner')).toBeVisible({ timeout: 3000 });
    await expect(page.getByText(/Yolo mode/i)).toBeVisible();
  });

  test('active yolo banner has revoke button', async ({ page }) => {
    await emitEvent(page, 'yolo:approved', {
      active: true,
      approved_at_ms: Date.now(),
      expires_at_ms: Date.now() + 300000,
      remaining_secs: 300,
      pending_request: null,
    });
    await expect(page.locator('.yolo-revoke-btn')).toBeVisible({ timeout: 3000 });
    await expect(page.getByText('Revoke')).toBeVisible();
  });

  test('yolo revoked event hides active banner', async ({ page }) => {
    // First show active banner
    await emitEvent(page, 'yolo:approved', {
      active: true,
      approved_at_ms: Date.now(),
      expires_at_ms: Date.now() + 300000,
      remaining_secs: 300,
      pending_request: null,
    });
    await expect(page.locator('.yolo-active-banner')).toBeVisible({ timeout: 3000 });

    // Then revoke
    await emitEvent(page, 'yolo:revoked', {});
    await expect(page.locator('.yolo-active-banner')).not.toBeVisible({ timeout: 3000 });
  });
});
