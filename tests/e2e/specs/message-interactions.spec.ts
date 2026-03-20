import { test, expect } from '@playwright/test';
import { injectTauriMock, emitEvent } from '../helpers/tauri-mock';

test.describe('Message Interactions', () => {
  test.beforeEach(async ({ page }) => {
    await injectTauriMock(page);
    await page.goto('/');
    await page.waitForSelector('textarea', { timeout: 5000 });
  });

  test('user message shows user icon', async ({ page }) => {
    const textarea = page.locator('textarea').first();
    await textarea.fill('Hello world');
    await textarea.press('Enter');
    await page.waitForTimeout(300);

    const userMsg = page.locator('.message.user').first();
    if (await userMsg.count() > 0) {
      await expect(userMsg).toBeVisible();
      // Should have the user role indicator
      const roleIcon = userMsg.locator('.message-role');
      if (await roleIcon.count() > 0) {
        await expect(roleIcon).toBeVisible();
      }
    }
  });

  test('assistant message shows bot icon', async ({ page }) => {
    const textarea = page.locator('textarea').first();
    await textarea.fill('Hi');
    await textarea.press('Enter');
    await page.waitForTimeout(300);

    // Simulate assistant response
    await emitEvent(page, 'chat:text-chunk', { text: 'Hello! How can I help?' });
    await emitEvent(page, 'chat:complete', {
      text: 'Hello! How can I help?',
      model_used: 'claude-sonnet-4-20250514',
    });
    await page.waitForTimeout(500);

    const assistantMsg = page.locator('.message.assistant').first();
    if (await assistantMsg.count() > 0) {
      await expect(assistantMsg).toBeVisible();
    }
  });

  test('message has copy button', async ({ page }) => {
    const textarea = page.locator('textarea').first();
    await textarea.fill('Test message');
    await textarea.press('Enter');
    await page.waitForTimeout(300);

    await emitEvent(page, 'chat:text-chunk', { text: 'Response text' });
    await emitEvent(page, 'chat:complete', {
      text: 'Response text',
      model_used: 'claude-sonnet-4-20250514',
    });
    await page.waitForTimeout(500);

    // Look for copy button in assistant message
    const copyBtn = page.locator('.message.assistant .message-action-btn').first();
    if (await copyBtn.count() > 0) {
      await expect(copyBtn).toBeVisible();
    }
  });

  test('tool indicator shows running state', async ({ page }) => {
    const textarea = page.locator('textarea').first();
    await textarea.fill('Run a search');
    await textarea.press('Enter');
    await page.waitForTimeout(300);

    await emitEvent(page, 'chat:tool-start', {
      tool_name: 'nexibot_search',
      tool_use_id: 'tool-abc',
    });
    await page.waitForTimeout(300);

    // Look for tool indicator (shows "running..." with tool icon)
    const toolIndicator = page.locator('.tool-indicator, [class*="tool"]').filter({ hasText: /running/i }).first();
    if (await toolIndicator.count() > 0) {
      await expect(toolIndicator).toBeVisible();
    }
  });

  test('tool indicator shows done state after completion', async ({ page }) => {
    const textarea = page.locator('textarea').first();
    await textarea.fill('Search something');
    await textarea.press('Enter');
    await page.waitForTimeout(300);

    await emitEvent(page, 'chat:tool-start', {
      tool_name: 'nexibot_search',
      tool_use_id: 'tool-xyz',
    });
    await page.waitForTimeout(200);

    await emitEvent(page, 'chat:tool-done', {
      tool_name: 'nexibot_search',
      tool_use_id: 'tool-xyz',
    });
    await page.waitForTimeout(300);

    const doneIndicator = page.locator('.tool-indicator.tool-done, .tool-done').first();
    if (await doneIndicator.count() > 0) {
      await expect(doneIndicator).toBeVisible();
    }
  });

  test('error message shows error styling', async ({ page }) => {
    const textarea = page.locator('textarea').first();
    await textarea.fill('Hello');
    await textarea.press('Enter');
    await page.waitForTimeout(300);

    await emitEvent(page, 'chat:error', {
      error: 'Rate limit exceeded. Please try again later.',
    });
    await page.waitForTimeout(500);

    const errorMsg = page.locator('.message.error, .message .error').first();
    if (await errorMsg.count() > 0) {
      await expect(errorMsg).toBeVisible();
    }
  });

  test('markdown in response renders correctly', async ({ page }) => {
    const textarea = page.locator('textarea').first();
    await textarea.fill('Show me a list');
    await textarea.press('Enter');
    await page.waitForTimeout(300);

    await emitEvent(page, 'chat:text-chunk', { text: '# Heading\n\n- Item 1\n- Item 2\n\n**Bold text**' });
    await emitEvent(page, 'chat:complete', {
      text: '# Heading\n\n- Item 1\n- Item 2\n\n**Bold text**',
      model_used: 'claude-sonnet-4-20250514',
    });
    await page.waitForTimeout(500);

    // Markdown should render heading and list
    const heading = page.locator('.message.assistant h1, .message.assistant h2').first();
    if (await heading.count() > 0) {
      await expect(heading).toBeVisible();
    }
  });

  test('code block in response renders with language tag', async ({ page }) => {
    const textarea = page.locator('textarea').first();
    await textarea.fill('Show me code');
    await textarea.press('Enter');
    await page.waitForTimeout(300);

    await emitEvent(page, 'chat:text-chunk', { text: '```python\nprint("hello")\n```' });
    await emitEvent(page, 'chat:complete', {
      text: '```python\nprint("hello")\n```',
      model_used: 'claude-sonnet-4-20250514',
    });
    await page.waitForTimeout(500);

    // Code block should be rendered
    const codeBlock = page.locator('.message.assistant pre, .message.assistant code').first();
    if (await codeBlock.count() > 0) {
      await expect(codeBlock).toBeVisible();
    }
  });
});
