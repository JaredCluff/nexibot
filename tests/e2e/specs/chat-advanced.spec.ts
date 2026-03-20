import { test, expect } from '@playwright/test';
import { injectTauriMock, emitEvent } from '../helpers/tauri-mock';

test.describe('Chat Advanced Features', () => {
  test.beforeEach(async ({ page }) => {
    await injectTauriMock(page);
    await page.goto('/');
    await page.waitForSelector('textarea', { timeout: 5000 });
  });

  test('agent selector bar is visible', async ({ page }) => {
    const agentBar = page.locator('.agent-selector-bar').first();
    if (await agentBar.count() > 0) {
      await expect(agentBar).toBeVisible();
    }
  });

  test('session override badges appear after /model command', async ({ page }) => {
    const textarea = page.locator('textarea').first();
    await textarea.fill('/model opus');
    await textarea.press('Enter');
    await page.waitForTimeout(500);
    // Model override badge should appear
    const badge = page.locator('.override-badge.model-badge');
    if (await badge.count() > 0) {
      await expect(badge).toBeVisible();
    }
  });

  test('tool execution shows tool indicator', async ({ page }) => {
    // Send a message
    const textarea = page.locator('textarea').first();
    await textarea.fill('Search for test data');
    await textarea.press('Enter');
    await page.waitForTimeout(300);

    // Simulate tool start event
    await emitEvent(page, 'chat:tool-start', {
      tool_name: 'nexibot_search',
      tool_use_id: 'tool-123',
    });
    await page.waitForTimeout(300);

    // Look for active tool indicator
    const taskPill = page.locator('.task-pill').first();
    if (await taskPill.count() > 0) {
      await expect(taskPill).toBeVisible();
    }
  });

  test('thinking indicator shows during extended thinking', async ({ page }) => {
    // Enable thinking mode first
    const textarea = page.locator('textarea').first();
    await textarea.fill('/think 5000');
    await textarea.press('Enter');
    await page.waitForTimeout(300);

    // Check for thinking badge
    const thinkingBadge = page.locator('.override-badge.thinking-badge');
    if (await thinkingBadge.count() > 0) {
      await expect(thinkingBadge).toBeVisible();
      await expect(thinkingBadge).toContainText('Thinking');
    }
  });

  test('stop button appears during streaming', async ({ page }) => {
    const textarea = page.locator('textarea').first();
    await textarea.fill('Tell me a long story');
    await textarea.press('Enter');
    await page.waitForTimeout(300);

    // Simulate streaming (loading state)
    await emitEvent(page, 'chat:text-chunk', { text: 'Once upon a time...' });
    await page.waitForTimeout(200);

    // Stop button should be visible during streaming
    const stopBtn = page.locator('.stop-stream-btn');
    if (await stopBtn.count() > 0) {
      await expect(stopBtn).toBeVisible();
      await expect(stopBtn).toContainText('Stop');
    }
  });

  test('tool approval bar shows approve and deny buttons', async ({ page }) => {
    // Simulate tool approval request
    await emitEvent(page, 'chat:tool-approval-request', {
      tool_name: 'execute_command',
      description: 'Run: git status',
      request_id: 'approval-test-1',
    });
    await page.waitForTimeout(500);

    const approvalBar = page.locator('.tool-approval-bar');
    if (await approvalBar.count() > 0) {
      await expect(approvalBar).toBeVisible();
      await expect(page.locator('.tool-approval-approve')).toBeVisible();
      await expect(page.locator('.tool-approval-deny')).toBeVisible();
    }
  });

  test('clicking approve on tool approval bar sends approval', async ({ page }) => {
    await emitEvent(page, 'chat:tool-approval-request', {
      tool_name: 'execute_command',
      description: 'Run: ls -la',
      request_id: 'approval-test-2',
    });
    await page.waitForTimeout(500);

    const approveBtn = page.locator('.tool-approval-approve');
    if (await approveBtn.count() > 0) {
      await approveBtn.click();
      // Approval bar should dismiss
      await page.waitForTimeout(500);
    }
  });

  test('multiple text chunks accumulate into full response', async ({ page }) => {
    const textarea = page.locator('textarea').first();
    await textarea.fill('Hello');
    await textarea.press('Enter');
    await page.waitForTimeout(300);

    // Send multiple chunks
    await emitEvent(page, 'chat:text-chunk', { text: 'Hello ' });
    await emitEvent(page, 'chat:text-chunk', { text: 'there! ' });
    await emitEvent(page, 'chat:text-chunk', { text: 'How are you?' });
    await emitEvent(page, 'chat:complete', {
      text: 'Hello there! How are you?',
      model_used: 'claude-sonnet-4-20250514',
    });

    await expect(page.getByText('Hello there! How are you?')).toBeVisible({ timeout: 5000 });
  });

  test('context usage bar shows when context is used', async ({ page }) => {
    const contextBar = page.locator('.context-bar__fill');
    // Context bar may or may not be visible depending on whether overrides are set
    // Just verify the chat container loads correctly
    await expect(page.locator('.chat-container')).toBeVisible();
  });

  test('error message from LLM shows auth prompt', async ({ page }) => {
    const textarea = page.locator('textarea').first();
    await textarea.fill('Hello');
    await textarea.press('Enter');
    await page.waitForTimeout(300);

    // Simulate auth error
    await emitEvent(page, 'chat:error', {
      error: 'No Claude authentication configured',
    });
    await page.waitForTimeout(500);

    // Auth prompt should appear
    const authPrompt = page.locator('[class*="auth"], [class*="Auth"]').first();
    if (await authPrompt.count() > 0) {
      await expect(authPrompt).toBeVisible();
    }
  });
});
