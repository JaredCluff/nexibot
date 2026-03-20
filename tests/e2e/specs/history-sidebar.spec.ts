import { test, expect } from '@playwright/test';
import { injectTauriMock } from '../helpers/tauri-mock';

const now = new Date().toISOString();
const yesterday = new Date(Date.now() - 86400000).toISOString();
const lastWeek = new Date(Date.now() - 5 * 86400000).toISOString();

const SESSION_DATA = [
  { session_id: 'sess-1', title: 'Building a REST API', started_at: now, last_active: now, message_count: 12 },
  { session_id: 'sess-2', title: 'Debugging CSS layout', started_at: yesterday, last_active: yesterday, message_count: 8 },
  { session_id: 'sess-3', title: 'Learning Rust', started_at: lastWeek, last_active: lastWeek, message_count: 25 },
];

test.describe('History Sidebar (Deep)', () => {
  test.beforeEach(async ({ page }) => {
    await injectTauriMock(page, undefined, {
      list_conversation_sessions: SESSION_DATA,
    });
    await page.goto('/');
    await page.waitForSelector('textarea', { timeout: 5000 });
  });

  test('sidebar shows session list when opened', async ({ page }) => {
    const hamburger = page.locator('button').first();
    await hamburger.click();
    await expect(page.locator('.history-sidebar')).toBeVisible({ timeout: 3000 });
    await expect(page.getByText('Building a REST API')).toBeVisible();
    await expect(page.getByText('Debugging CSS layout')).toBeVisible();
    await expect(page.getByText('Learning Rust')).toBeVisible();
  });

  test('sessions are grouped by date', async ({ page }) => {
    const hamburger = page.locator('button').first();
    await hamburger.click();
    await expect(page.locator('.history-sidebar')).toBeVisible({ timeout: 3000 });
    await expect(page.getByText('Today')).toBeVisible();
  });

  test('sessions show message count', async ({ page }) => {
    const hamburger = page.locator('button').first();
    await hamburger.click();
    await expect(page.locator('.history-sidebar')).toBeVisible({ timeout: 3000 });
    await expect(page.getByText('12 msgs')).toBeVisible();
    await expect(page.getByText('8 msgs')).toBeVisible();
  });

  test('new conversation button is present', async ({ page }) => {
    const hamburger = page.locator('button').first();
    await hamburger.click();
    await expect(page.locator('.new-conversation-btn')).toBeVisible({ timeout: 3000 });
    await expect(page.getByText('New Conversation')).toBeVisible();
  });

  test('sidebar has History header', async ({ page }) => {
    const hamburger = page.locator('button').first();
    await hamburger.click();
    await expect(page.locator('.sidebar-title')).toBeVisible({ timeout: 3000 });
    await expect(page.getByText('History')).toBeVisible();
  });

  test('session items are clickable', async ({ page }) => {
    const hamburger = page.locator('button').first();
    await hamburger.click();
    await expect(page.locator('.history-sidebar')).toBeVisible({ timeout: 3000 });
    const sessionItem = page.locator('.session-item').first();
    await expect(sessionItem).toBeVisible();
    await sessionItem.click();
  });
});
