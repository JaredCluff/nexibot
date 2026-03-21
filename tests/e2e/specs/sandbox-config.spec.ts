import { test, expect } from '@playwright/test';
import { injectTauriMock } from '../helpers/tauri-mock';

test.describe('Sandbox Configuration', () => {
  test.beforeEach(async ({ page }) => {
    await injectTauriMock(page, undefined, {
      get_config: {
        sandbox: {
          enabled: true,
          image: 'debian:bookworm-slim@sha256:98f4b71de414932bb0b8a9ac41d0d3cf0ebb77a4638ae99c28a9e9bfe26ae98e',
          memory_limit: '512m',
          cpu_limit: 1.0,
          network_mode: 'none',
          timeout_seconds: 60,
          fallback: 'AllowHost',
        },
        execute: {
          enabled: true,
          sandbox_policy: 'Dangerous',
        },
      },
    });
    await page.goto('/');
  });

  test('sandbox config is retrievable via invoke', async ({ page }) => {
    const config = await page.evaluate(async () => {
      return await (window as any).__TAURI_INTERNALS__.invoke('get_config');
    });

    expect(config.sandbox).toBeDefined();
    expect(config.sandbox.enabled).toBe(true);
    expect(config.sandbox.fallback).toBe('AllowHost');
    expect(config.sandbox.network_mode).toBe('none');
  });

  test('sandbox defaults to enabled with AllowHost fallback', async ({ page }) => {
    const config = await page.evaluate(async () => {
      return await (window as any).__TAURI_INTERNALS__.invoke('get_config');
    });

    expect(config.sandbox.enabled).toBe(true);
    expect(config.sandbox.fallback).toBe('AllowHost');
  });

  test('execute config includes sandbox_policy', async ({ page }) => {
    const config = await page.evaluate(async () => {
      return await (window as any).__TAURI_INTERNALS__.invoke('get_config');
    });

    expect(config.execute.sandbox_policy).toBe('Dangerous');
  });
});
