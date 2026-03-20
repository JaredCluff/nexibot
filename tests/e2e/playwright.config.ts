import { defineConfig, devices } from '@playwright/test';

/**
 * Playwright configuration for NexiBot E2E tests.
 *
 * Tests run against the Vite dev server with a mock Tauri IPC layer
 * that routes invoke() calls to the NexiBot HTTP API server.
 *
 * Prerequisites:
 *   1. Mock LLM server running on :18799
 *   2. NexiBot backend running (with NEXIBOT_LLM_BASE_URL=http://127.0.0.1:18799)
 *   3. Vite dev server running on :5173
 */
export default defineConfig({
  testDir: './specs',
  fullyParallel: false, // Sequential — tests may share backend state
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  workers: 1,
  reporter: process.env.CI ? 'github' : 'html',
  timeout: 30_000,

  use: {
    baseURL: 'http://localhost:5173',
    trace: 'on-first-retry',
    screenshot: 'only-on-failure',
    video: 'retain-on-failure',
  },

  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
  ],

  // Start Vite dev server before tests (if not already running)
  webServer: [
    {
      command: 'cd ../../ui && npm run dev',
      port: 5173,
      reuseExistingServer: true,
      timeout: 30_000,
    },
  ],
});
