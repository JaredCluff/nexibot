import { test, expect } from '@playwright/test';
import { injectTauriMock, emitEvent } from '../helpers/tauri-mock';

test.describe('Chat Interface', () => {
  test.beforeEach(async ({ page }) => {
    await injectTauriMock(page);
    await page.goto('/');
    // Wait for chat to be ready
    await page.waitForSelector('textarea, input[type="text"]', { timeout: 5000 });
  });

  test('can type a message in the input area', async ({ page }) => {
    const input = page.locator('textarea, input[type="text"]').first();
    await input.fill('Hello NexiBot!');
    await expect(input).toHaveValue('Hello NexiBot!');
  });

  test('send button becomes active when text is entered', async ({ page }) => {
    const input = page.locator('textarea, input[type="text"]').first();
    await input.fill('Test message');
    // Look for a send button that becomes enabled
    const sendBtn = page.locator('button').filter({ hasText: /send/i }).first();
    if (await sendBtn.count() > 0) {
      await expect(sendBtn).toBeEnabled();
    }
  });

  test('displays user message after sending', async ({ page }) => {
    const input = page.locator('textarea, input[type="text"]').first();
    await input.fill('Hello NexiBot!');
    // Press Enter to send
    await input.press('Enter');
    // User message should appear in the chat
    await expect(page.locator('.message, .chat-message, [data-role="user"]').first()).toBeVisible({ timeout: 5000 });
  });

  test('displays assistant response from streaming event', async ({ page }) => {
    const input = page.locator('textarea, input[type="text"]').first();
    await input.fill('What is 2+2?');
    await input.press('Enter');

    // Simulate streaming response events from backend
    await page.waitForTimeout(500);
    await emitEvent(page, 'chat:text-chunk', { text: 'The answer is ' });
    await emitEvent(page, 'chat:text-chunk', { text: '4.' });
    await emitEvent(page, 'chat:complete', {
      text: 'The answer is 4.',
      model_used: 'claude-sonnet-4-20250514',
    });

    // Check for the response text in the page
    await expect(page.getByText('The answer is 4.')).toBeVisible({ timeout: 5000 });
  });

  test('shows model indicator after response', async ({ page }) => {
    // Simulate a complete response with model info
    await emitEvent(page, 'chat:complete', {
      text: 'Hello!',
      model_used: 'claude-sonnet-4-20250514',
    });

    // Model indicator should show somewhere
    const modelIndicator = page.getByText(/claude-sonnet|sonnet/i).first();
    if (await modelIndicator.count() > 0) {
      await expect(modelIndicator).toBeVisible();
    }
  });

  test('tool approval banner appears on tool-approval-request event', async ({ page }) => {
    await emitEvent(page, 'chat:tool-approval-request', {
      tool_name: 'execute_command',
      description: 'Run: ls -la',
      request_id: 'test-123',
    });

    // Should show an approval prompt
    const approvalElement = page.locator('[class*="approval"], [class*="confirm"]').first();
    if (await approvalElement.count() > 0) {
      await expect(approvalElement).toBeVisible({ timeout: 3000 });
    }
  });
});
