import { test, expect } from '@playwright/test';
import { injectTauriMock } from '../helpers/tauri-mock';

const AUTH_OVERRIDES = {
  get_provider_status: { anthropic_configured: false, openai_configured: false },
  get_config: {
    claude: { model: 'claude-sonnet-4-20250514', system_prompt: '', api_key: '' },
    openai: { api_key: '' },
    k2k: { enabled: false, local_agent_url: '', client_id: '' },
    audio: { enabled: false, sample_rate: 16000, channels: 1 },
    wakeword: { enabled: false, wake_word: 'hey_nexibot', threshold: 0.5 },
  },
  update_config: null,
  start_oauth_flow: 'https://example.com/oauth',
  open_oauth_browser: 'https://example.com/oauth',
  complete_oauth_flow: null,
  check_subscription: 'Active',
  get_subscription_credentials: 'sk-test-key-123',
  start_openai_device_flow: { user_code: 'TEST-CODE', verification_uri: 'https://example.com/verify', interval: 5 },
  poll_openai_device_flow: { status: 'complete', error: null },
  validate_provider_models: [{ id: 'cerebras/gpt-oss-120b', size_score: 100 }],
};

test.describe('Auth Prompt', () => {
  test.beforeEach(async ({ page }) => {
    await injectTauriMock(page, undefined, AUTH_OVERRIDES);
  });

  test('auth prompt appears when no provider is configured', async ({ page }) => {
    await page.goto('/');
    await expect(page.getByText(/Authentication Required|How would you like to connect/i).first()).toBeVisible({ timeout: 5000 });
  });

  test('auth prompt shows Sign In and API Key options', async ({ page }) => {
    await page.goto('/');
    await expect(page.getByText('Sign In').first()).toBeVisible({ timeout: 5000 });
    await expect(page.getByText('API Key').first()).toBeVisible();
  });

  test('Sign In path shows provider choices', async ({ page }) => {
    await page.goto('/');
    await page.locator('.provider-option.recommended').click();
    await expect(page.getByText(/Claude|Anthropic/i).first()).toBeVisible({ timeout: 3000 });
    await expect(page.getByText(/ChatGPT|OpenAI/i).first()).toBeVisible();
    await expect(page.getByText('Knowledge Nexus').first()).toBeVisible();
  });

  test('API Key path shows Claude, OpenAI, and Cerebras options', async ({ page }) => {
    await page.goto('/');
    await page.locator('.provider-option').nth(1).click();
    await page.waitForTimeout(300);
    await expect(page.getByText('Anthropic').first()).toBeVisible({ timeout: 3000 });
    await expect(page.getByText('OpenAI').first()).toBeVisible();
    await expect(page.getByText('Cerebras').first()).toBeVisible();
  });

  test('Claude API key entry shows input field', async ({ page }) => {
    await page.goto('/');
    await page.locator('.provider-option').nth(1).click();
    await page.waitForTimeout(300);
    await page.locator('.provider-option').filter({ hasText: 'Anthropic' }).first().click();
    await page.waitForTimeout(300);
    await expect(page.getByText('Enter Your Anthropic API Key')).toBeVisible({ timeout: 3000 });
    await expect(page.locator('input[type="password"]')).toBeVisible();
  });

  test('OpenAI API key entry shows input field', async ({ page }) => {
    await page.goto('/');
    await page.locator('.provider-option').nth(1).click();
    await page.waitForTimeout(300);
    await page.locator('.provider-option').filter({ hasText: 'OpenAI' }).first().click();
    await page.waitForTimeout(300);
    await expect(page.getByText('Enter Your OpenAI API Key')).toBeVisible({ timeout: 3000 });
  });

  test('Cerebras API key entry shows input field', async ({ page }) => {
    await page.goto('/');
    await page.locator('.provider-option').nth(1).click();
    await page.waitForTimeout(300);
    await page.locator('.provider-option').filter({ hasText: 'Cerebras' }).first().click();
    await page.waitForTimeout(300);
    await expect(page.getByText('Enter Your Cerebras API Key')).toBeVisible({ timeout: 3000 });
  });

  test('OpenAI device code flow shows Start Sign In button', async ({ page }) => {
    await page.goto('/');
    await page.locator('.provider-option.recommended').click();
    await page.waitForTimeout(300);
    await page.locator('.provider-option').filter({ hasText: /ChatGPT|OpenAI/i }).first().click();
    await page.waitForTimeout(300);
    await expect(page.getByText('Start Sign In')).toBeVisible({ timeout: 3000 });
  });

  test('Knowledge Nexus path shows subscription options', async ({ page }) => {
    await page.goto('/');
    await page.locator('.provider-option.recommended').click();
    await page.waitForTimeout(300);
    await page.locator('.provider-option').filter({ hasText: 'Knowledge Nexus' }).first().click();
    await page.waitForTimeout(300);
    await expect(page.getByText('Knowledge Nexus').first()).toBeVisible({ timeout: 3000 });
    await expect(page.getByText(/Claude.*Anthropic/i).first()).toBeVisible();
    await expect(page.getByText(/GPT-4o.*OpenAI/i).first()).toBeVisible();
  });

  test('Back button works from provider selection', async ({ page }) => {
    await page.goto('/');
    await page.locator('.provider-option.recommended').click();
    await page.waitForTimeout(300);
    const backBtn = page.locator('button.secondary').filter({ hasText: 'Back' });
    await backBtn.click();
    await page.waitForTimeout(300);
    await expect(page.getByText('Sign In').first()).toBeVisible({ timeout: 3000 });
    await expect(page.getByText('API Key').first()).toBeVisible();
  });
});
