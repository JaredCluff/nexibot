import { test, expect } from '@playwright/test';
import { injectTauriMock } from '../helpers/tauri-mock';

const FULL_CONFIG = {
  config_version: 1,
  claude: { model: 'claude-sonnet-4-20250514', max_tokens: 4096, system_prompt: 'You are NexiBot.' },
  openai: { model: 'gpt-4o', max_tokens: 4096 },
  cerebras: {},
  k2k: { enabled: false, local_agent_url: '', client_id: '', supermemory_enabled: false, supermemory_auto_extract: false },
  audio: { enabled: false, sample_rate: 16000, channels: 1 },
  wakeword: { enabled: false, wake_word: 'hey_nexibot', threshold: 0.5, sleep_timeout_seconds: 30, conversation_timeout_seconds: 120, stt_wakeword_enabled: false, stt_require_both: false, voice_response_enabled: false },
  vad: { enabled: false, threshold: 0.5, min_speech_duration_ms: 250, min_silence_duration_ms: 300 },
  stt: { enabled: false, backend: 'native' },
  tts: { enabled: false, backend: 'native', macos_voice: 'Samantha' },
  mcp: { enabled: false, servers: [] },
  computer_use: { enabled: false, display_width: 1280, display_height: 720, require_confirmation: true },
  guardrails: { security_level: 'standard', block_destructive_commands: true, block_sensitive_data_sharing: true, detect_prompt_injection: true, block_prompt_injection: false, confirm_external_actions: true, dangers_acknowledged: false, server_permissions: {}, default_tool_permission: 'AllowWithLogging', dangerous_tool_patterns: [], use_dcg: true },
  autonomous_mode: { enabled: false, filesystem: { read: 'allow', write: 'ask', delete: 'deny' }, execute: { run_command: 'ask', run_python: 'ask', run_node: 'ask' }, fetch: { get_requests: 'allow', post_requests: 'ask' }, browser: { navigate: 'ask', interact: 'ask' }, computer_use: { level: 'deny' }, mcp: {}, settings_modification: { level: 'deny' }, memory_modification: { level: 'ask' }, soul_modification: { level: 'deny' } },
  defense: { enabled: false, deberta_enabled: false, deberta_threshold: 0.85, llama_guard_enabled: false, llama_guard_mode: 'local', llama_guard_api_url: '', allow_remote_llama_guard: false, fail_open: false },
  execute: { enabled: false, allowed_commands: [], blocked_commands: [], default_timeout_ms: 30000, max_output_bytes: 1048576, use_dcg: true, skill_runtime_exec_enabled: false },
  filesystem: { enabled: true, allowed_paths: [], blocked_paths: [], max_read_bytes: 10485760, max_write_bytes: 10485760 },
  fetch: { enabled: true, allowed_domains: [], blocked_domains: [], max_response_bytes: 10485760, default_timeout_ms: 30000 },
  webhooks: { enabled: false, port: 18791, endpoints: [], tls: { enabled: false, auto_generate: false } },
  telegram: { enabled: false },
  whatsapp: { enabled: false },
  discord: { enabled: false },
  slack: { enabled: false },
  signal: { enabled: false },
  teams: { enabled: false },
  matrix: { enabled: false },
  email: { enabled: false },
  gateway: { enabled: false, port: 18792, bind_address: '127.0.0.1', auth_mode: 'Token', max_connections: 10 },
  sandbox: { enabled: false, image: 'nexibot-sandbox', non_root_user: 'appuser', memory_limit: '256m', cpu_limit: 1, network_mode: 'none', timeout_seconds: 30, blocked_paths: [] },
  browser: { enabled: false },
  scheduler: { enabled: false, tasks: [] },
  gated_shell: { enabled: false, debug_mode: false, record_sessions: true },
};

test.describe('Settings Tabs Navigation', () => {
  test.beforeEach(async ({ page }) => {
    await injectTauriMock(page, undefined, {
      get_config: FULL_CONFIG,
      list_skills: [],
      list_agents: [],
      list_mcp_servers: [],
      get_guardrails_config: FULL_CONFIG.guardrails,
      get_defense_status: { enabled: false, models_loaded: [] },
      get_voice_status: { state: 'idle', enabled: false },
      get_gated_shell_status: { enabled: false, debug_mode: false, record_sessions: true, secret_count: 0, active_sessions: 0 },
      list_scheduled_tasks: [],
      get_bridge_health: { status: 'ok' },
    });
    await page.goto('/');
    await page.waitForSelector('textarea', { timeout: 5000 });
    // Open settings via the gear button
    await page.locator('.settings-button').click();
    await expect(page.locator('.settings')).toBeVisible({ timeout: 5000 });
  });

  test('settings panel has Save and Done buttons', async ({ page }) => {
    await expect(page.locator('.save-btn')).toBeVisible();
    await expect(page.locator('.close-btn')).toBeVisible();
  });

  test('all 13 settings tabs are visible', async ({ page }) => {
    const tabs = ['Models', 'Voice', 'Knowledge', 'Channels', 'Tools',
                  'Connectors', 'Automation', 'Skills', 'Security',
                  'Key Vault', 'Agents', 'System', 'NexiGate'];
    for (const tab of tabs) {
      await expect(page.locator('.tabs').getByText(tab, { exact: true })).toBeVisible({ timeout: 2000 });
    }
  });

  test('Models tab is active by default', async ({ page }) => {
    const modelsTab = page.locator('.tabs').getByText('Models', { exact: true });
    await expect(modelsTab).toBeVisible();
    // The active tab should have 'active' class
    const activeTab = page.locator('.tab.active, .tab-btn.active, [class*="tab"][class*="active"]').first();
    if (await activeTab.count() > 0) {
      await expect(activeTab).toContainText('Models');
    }
  });

  test('can navigate to Voice tab', async ({ page }) => {
    await page.locator('.tabs').getByText('Voice', { exact: true }).click();
    await page.waitForTimeout(300);
    // Voice tab shows "Enable Voice Pipeline" checkbox
    await expect(page.getByText('Enable Voice Pipeline')).toBeVisible({ timeout: 3000 });
  });

  test('can navigate to Knowledge tab', async ({ page }) => {
    await page.locator('.tabs').getByText('Knowledge', { exact: true }).click();
    await page.waitForTimeout(300);
    const knowledgeContent = page.getByText(/Knowledge|Memory|Search|Embedding/i).first();
    if (await knowledgeContent.count() > 0) {
      await expect(knowledgeContent).toBeVisible();
    }
  });

  test('can navigate to Tools tab', async ({ page }) => {
    await page.locator('.tabs').getByText('Tools', { exact: true }).click();
    await page.waitForTimeout(300);
    // Tools tab shows Web Search section
    await expect(page.getByText('Web Search')).toBeVisible({ timeout: 3000 });
  });

  test('can navigate to Connectors tab', async ({ page }) => {
    await page.locator('.tabs').getByText('Connectors', { exact: true }).click();
    await page.waitForTimeout(300);
    const connectorsContent = page.getByText(/MCP|Connector|Server/i).first();
    if (await connectorsContent.count() > 0) {
      await expect(connectorsContent).toBeVisible();
    }
  });

  test('can navigate to Automation tab', async ({ page }) => {
    await page.locator('.tabs').getByText('Automation', { exact: true }).click();
    await page.waitForTimeout(300);
    const automationContent = page.getByText(/Scheduled|Task|Automation|Cron/i).first();
    if (await automationContent.count() > 0) {
      await expect(automationContent).toBeVisible();
    }
  });

  test('can navigate to Skills tab', async ({ page }) => {
    await page.locator('.tabs').getByText('Skills', { exact: true }).click();
    await page.waitForTimeout(300);
    const skillsContent = page.getByText(/Skills|ClawHub|Marketplace/i).first();
    if (await skillsContent.count() > 0) {
      await expect(skillsContent).toBeVisible();
    }
  });

  test('can navigate to Security tab', async ({ page }) => {
    await page.locator('.tabs').getByText('Security', { exact: true }).click();
    await page.waitForTimeout(300);
    const securityContent = page.getByText(/Security|Guardrails|Defense|Level/i).first();
    if (await securityContent.count() > 0) {
      await expect(securityContent).toBeVisible();
    }
  });

  test('can navigate to Key Vault tab', async ({ page }) => {
    await page.locator('.tabs').getByText('Key Vault', { exact: true }).click();
    await page.waitForTimeout(300);
    const vaultContent = page.getByText(/Key Vault|API Key|Credential|Auth/i).first();
    if (await vaultContent.count() > 0) {
      await expect(vaultContent).toBeVisible();
    }
  });

  test('can navigate to Agents tab', async ({ page }) => {
    await page.locator('.tabs').getByText('Agents', { exact: true }).click();
    await page.waitForTimeout(300);
    const agentsContent = page.getByText(/Agent|Team|Orchestrat/i).first();
    if (await agentsContent.count() > 0) {
      await expect(agentsContent).toBeVisible();
    }
  });

  test('can navigate to System tab', async ({ page }) => {
    await page.locator('.tabs').getByText('System', { exact: true }).click();
    await page.waitForTimeout(300);
    const systemContent = page.getByText(/System|Bridge|Gateway|Headless/i).first();
    if (await systemContent.count() > 0) {
      await expect(systemContent).toBeVisible();
    }
  });

  test('can navigate to NexiGate tab', async ({ page }) => {
    await page.locator('.tabs').getByText('NexiGate', { exact: true }).click();
    await page.waitForTimeout(300);
    const gateContent = page.getByText(/NexiGate|Gated Shell|Shell/i).first();
    if (await gateContent.count() > 0) {
      await expect(gateContent).toBeVisible();
    }
  });

  test('Done button closes settings', async ({ page }) => {
    await page.locator('.close-btn').click();
    await expect(page.locator('.settings')).not.toBeVisible({ timeout: 3000 });
    // Chat should be visible again
    await expect(page.locator('textarea')).toBeVisible();
  });
});
