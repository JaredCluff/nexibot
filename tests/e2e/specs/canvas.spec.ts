import { test, expect } from '@playwright/test';
import { injectTauriMock, emitEvent } from '../helpers/tauri-mock';

test.describe('Canvas Panel', () => {
  test.beforeEach(async ({ page }) => {
    await injectTauriMock(page);
    await page.goto('/');
  });

  test('canvas opens when toggle button is clicked', async ({ page }) => {
    await page.locator('.canvas-toggle-button').click();
    const canvas = page.locator('[class*="canvas"], .canvas-panel').first();
    await expect(canvas).toBeVisible({ timeout: 3000 });
  });

  test('canvas opens when artifact event is received', async ({ page }) => {
    await emitEvent(page, 'canvas:push', {
      id: 'test-artifact-1',
      type: 'code',
      language: 'python',
      content: 'print("hello world")',
      title: 'Hello World',
    });

    await page.waitForTimeout(500);
    // Canvas should auto-open
    const canvas = page.locator('[class*="canvas"], .canvas-panel').first();
    await expect(canvas).toBeVisible({ timeout: 3000 });
  });

  test('canvas displays code artifact', async ({ page }) => {
    await emitEvent(page, 'canvas:push', {
      id: 'test-code-1',
      type: 'code',
      language: 'javascript',
      content: 'const x = 42;',
      title: 'JS Snippet',
    });

    await page.waitForTimeout(500);
    await expect(page.getByText('const x = 42')).toBeVisible({ timeout: 3000 });
  });

  test('canvas displays HTML artifact', async ({ page }) => {
    await emitEvent(page, 'canvas:push', {
      id: 'test-html-1',
      type: 'html',
      language: 'html',
      content: '<h1>Hello</h1>',
      title: 'HTML Preview',
    });

    await page.waitForTimeout(500);
    // Canvas panel should be open after the artifact event
    const canvas = page.locator('[class*="canvas"], .canvas-panel').first();
    await expect(canvas).toBeVisible({ timeout: 3000 });
  });
});
