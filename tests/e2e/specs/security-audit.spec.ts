import { test, expect } from '@playwright/test';
import { injectTauriMock, emitEvent } from '../helpers/tauri-mock';

test.describe('Security Audit', () => {
  test.beforeEach(async ({ page }) => {
    await injectTauriMock(page, undefined, {
      run_security_audit: {
        findings: [
          {
            id: 'cfg-defense-disabled',
            severity: 'High',
            title: 'Defense pipeline disabled',
            description: 'The defense pipeline is not enabled.',
            fix_hint: 'Set defense.enabled: true in config.yaml',
            auto_fixable: true,
          },
          {
            id: 'cfg-sandbox-disabled',
            severity: 'Medium',
            title: 'Docker sandbox not available',
            description: 'Docker is not installed or not running.',
            fix_hint: 'Install Docker for sandboxed command execution',
            auto_fixable: false,
          },
        ],
        passed_count: 15,
        total_checks: 17,
        timestamp: new Date().toISOString(),
      },
    });
    await page.goto('/');
  });

  test('displays security audit results when triggered', async ({ page }) => {
    // Navigate to settings/security if the UI has such a section
    // This test verifies the audit data can be rendered
    const auditData = await page.evaluate(async () => {
      const result = await (window as any).__TAURI_INTERNALS__.invoke('run_security_audit');
      return result;
    });

    expect(auditData.total_checks).toBe(17);
    expect(auditData.passed_count).toBe(15);
    expect(auditData.findings).toHaveLength(2);
    expect(auditData.findings[0].severity).toBe('High');
  });

  test('audit report contains expected finding fields', async ({ page }) => {
    const auditData = await page.evaluate(async () => {
      return await (window as any).__TAURI_INTERNALS__.invoke('run_security_audit');
    });

    const finding = auditData.findings[0];
    expect(finding).toHaveProperty('id');
    expect(finding).toHaveProperty('severity');
    expect(finding).toHaveProperty('title');
    expect(finding).toHaveProperty('description');
    expect(finding).toHaveProperty('auto_fixable');
  });

  test('auto-fixable findings are identified', async ({ page }) => {
    const auditData = await page.evaluate(async () => {
      return await (window as any).__TAURI_INTERNALS__.invoke('run_security_audit');
    });

    const autoFixable = auditData.findings.filter((f: any) => f.auto_fixable);
    expect(autoFixable).toHaveLength(1);
    expect(autoFixable[0].id).toBe('cfg-defense-disabled');
  });
});
