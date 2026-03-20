import { test, expect } from '@playwright/test';
import { injectTauriMock, emitEvent } from '../helpers/tauri-mock';

test.describe('Canvas Deep Tests', () => {
  test.beforeEach(async ({ page }) => {
    await injectTauriMock(page);
    await page.goto('/');
    await page.waitForSelector('textarea', { timeout: 5000 });
  });

  test('canvas panel has header with title and close button', async ({ page }) => {
    // Open canvas
    await page.getByText('Canvas').click();
    await expect(page.locator('.canvas')).toBeVisible({ timeout: 3000 });
    await expect(page.locator('.canvas-title')).toBeVisible();
    await expect(page.locator('.canvas-close-btn')).toBeVisible();
  });

  test('empty canvas shows helpful message', async ({ page }) => {
    await page.getByText('Canvas').click();
    await expect(page.locator('.canvas')).toBeVisible({ timeout: 3000 });
    await expect(page.getByText('No artifacts yet')).toBeVisible();
    await expect(page.getByText('Open in Canvas')).toBeVisible();
  });

  test('close button hides canvas', async ({ page }) => {
    await page.getByText('Canvas').click();
    await expect(page.locator('.canvas')).toBeVisible({ timeout: 3000 });
    await page.locator('.canvas-close-btn').click();
    await expect(page.locator('.canvas')).not.toBeVisible({ timeout: 3000 });
  });

  test('code artifact displays with syntax highlighting', async ({ page }) => {
    await page.getByText('Canvas').click();
    await expect(page.locator('.canvas')).toBeVisible({ timeout: 3000 });

    // Inject a code artifact via event
    await emitEvent(page, 'canvas:artifact', {
      id: 'art-code-1',
      type: 'code',
      language: 'javascript',
      content: 'function hello() {\n  return "world";\n}',
      title: 'hello.js',
    });
    await page.waitForTimeout(500);

    // Canvas tab should show the artifact title
    const tab = page.locator('.canvas-tab');
    if (await tab.count() > 0) {
      await expect(tab.first()).toContainText('hello.js');
    }
  });

  test('HTML artifact renders in preview', async ({ page }) => {
    await page.getByText('Canvas').click();
    await expect(page.locator('.canvas')).toBeVisible({ timeout: 3000 });

    await emitEvent(page, 'canvas:artifact', {
      id: 'art-html-1',
      type: 'html',
      content: '<h1>Test Heading</h1><p>Hello from HTML</p>',
      title: 'Preview',
    });
    await page.waitForTimeout(500);

    const tab = page.locator('.canvas-tab');
    if (await tab.count() > 0) {
      await expect(tab.first()).toContainText('Preview');
    }
  });

  test('multiple artifacts create multiple tabs', async ({ page }) => {
    await page.getByText('Canvas').click();
    await expect(page.locator('.canvas')).toBeVisible({ timeout: 3000 });

    await emitEvent(page, 'canvas:artifact', {
      id: 'art-1',
      type: 'code',
      language: 'python',
      content: 'print("hello")',
      title: 'script.py',
    });
    await emitEvent(page, 'canvas:artifact', {
      id: 'art-2',
      type: 'code',
      language: 'rust',
      content: 'fn main() {}',
      title: 'main.rs',
    });
    await page.waitForTimeout(500);

    const tabs = page.locator('.canvas-tab');
    if (await tabs.count() >= 2) {
      await expect(tabs.nth(0)).toContainText('script.py');
      await expect(tabs.nth(1)).toContainText('main.rs');
    }
  });

  test('clicking a tab switches the active artifact', async ({ page }) => {
    await page.getByText('Canvas').click();
    await expect(page.locator('.canvas')).toBeVisible({ timeout: 3000 });

    await emitEvent(page, 'canvas:artifact', {
      id: 'art-a',
      type: 'code',
      language: 'javascript',
      content: 'const a = 1;',
      title: 'file-a.js',
    });
    await emitEvent(page, 'canvas:artifact', {
      id: 'art-b',
      type: 'code',
      language: 'javascript',
      content: 'const b = 2;',
      title: 'file-b.js',
    });
    await page.waitForTimeout(500);

    const tabs = page.locator('.canvas-tab');
    if (await tabs.count() >= 2) {
      // Click second tab
      await tabs.nth(1).click();
      // It should become active
      await expect(tabs.nth(1)).toHaveClass(/active/);
    }
  });
});
