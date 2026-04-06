import { useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useSettings } from '../SettingsContext';
import { notifyError } from '../../../shared/notify';
import { InfoTip } from '../shared/InfoTip';
import { TagInput } from '../shared/TagInput';
import {
  PATTERN_SUGGESTIONS,
  SAFE_COMMAND_SUGGESTIONS,
  DANGEROUS_COMMAND_SUGGESTIONS,
  SENSITIVE_PATH_SUGGESTIONS,
  SAFE_PATH_SUGGESTIONS,
  ALLOWED_DOMAIN_SUGGESTIONS,
  BLOCKED_DOMAIN_SUGGESTIONS,
} from '../shared/suggestions';

interface SecurityAuditFinding {
  id: string;
  severity: string;
  title: string;
  description: string;
  fix_hint: string | null;
  auto_fixable: boolean;
}

interface SecurityAuditReport {
  findings: SecurityAuditFinding[];
  passed_count: number;
  total_checks: number;
  timestamp: string;
}
import AutonomousModePanel from '../../AutonomousModePanel';

export function SecurityTab() {
  const { config, setConfig, defenseStatus, toolPermissions, loadToolPermissions, mcpServers } = useSettings();

  const [savingPermServer, setSavingPermServer] = useState<string | null>(null);


  // Security Audit state
  const [auditReport, setAuditReport] = useState<SecurityAuditReport | null>(null);
  const [runningAudit, setRunningAudit] = useState(false);
  const [fixingId, setFixingId] = useState<string | null>(null);

  const handleRunAudit = async () => {
    setRunningAudit(true);
    setAuditReport(null);
    try {
      const reportJson = await invoke<string>('run_security_audit');
      const report = JSON.parse(reportJson) as SecurityAuditReport;
      setAuditReport(report);
    } catch (error) {
      notifyError('Security', `Audit failed: ${error}`);
    } finally {
      setRunningAudit(false);
    }
  };

  const handleAutoFix = async (findingId: string) => {
    setFixingId(findingId);
    try {
      await invoke('auto_fix_finding', { findingId });
      handleRunAudit();
    } catch (error) {
      notifyError('Security', `Auto-fix failed: ${error}`);
    } finally {
      setFixingId(null);
    }
  };

  const severityOrder: Record<string, number> = { Critical: 0, High: 1, Medium: 2, Low: 3, Info: 4 };

  if (!config) return null;

  return (
    <div className="tab-content">
      {/* Autonomous Mode */}
      <div className="settings-group">
        <h3>Autonomous Mode <InfoTip text="Controls what NexiBot can do without asking. This is an overlay on top of guardrails — it can grant permission within guardrail bounds, but never bypass safety checks." /></h3>
        <p className="group-description">
          Configure what the agent can do without asking. This is an overlay on top of guardrails —
          it can make things more permissive within guardrail bounds, but never less safe.
        </p>
        <AutonomousModePanel />
      </div>

      {/* Security / Guardrails */}
      <div className="settings-group">
        <h3>Security <InfoTip text="Safety controls that prevent the AI from executing dangerous operations or leaking sensitive data." /></h3>
        <p className="group-description">Control how guardrails protect against dangerous tool calls and prompt injection.</p>

        <label className="field">
          <span>Security Level <InfoTip text="Overall protection level. Standard is recommended for most users. Maximum blocks many operations; Disabled removes all protection." /></span>
          <select
            value={config.guardrails.security_level}
            onChange={(e) => setConfig({
              ...config,
              guardrails: { ...config.guardrails, security_level: e.target.value },
            })}
          >
            <option value="Maximum">Maximum — blocks many operations</option>
            <option value="Standard">Standard — balanced (recommended)</option>
            <option value="Relaxed">Relaxed — warnings only</option>
            <option value="Disabled">Disabled — no protection</option>
          </select>
        </label>
        {(config.guardrails.security_level === 'Relaxed' || config.guardrails.security_level === 'Disabled') && (
          <div className="warning-banner">
            {config.guardrails.security_level === 'Disabled'
              ? 'All security protections are disabled. NexiBot will execute any tool call without checks.'
              : 'Security is relaxed. Dangerous operations will only generate warnings, not be blocked.'}
          </div>
        )}

        <div className="inline-toggle">
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={config.guardrails.block_destructive_commands}
              onChange={(e) => setConfig({
                ...config,
                guardrails: { ...config.guardrails, block_destructive_commands: e.target.checked },
              })}
            />
            Block destructive commands (rm -rf, DROP TABLE, etc.) <InfoTip text="Prevent commands like rm -rf, DROP TABLE, and other irreversible operations." />
          </label>
        </div>
        <div className="inline-toggle">
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={config.guardrails.block_sensitive_data_sharing}
              onChange={(e) => setConfig({
                ...config,
                guardrails: { ...config.guardrails, block_sensitive_data_sharing: e.target.checked },
              })}
            />
            Block sensitive data sharing <InfoTip text="Detect and prevent accidental sharing of passwords, API keys, and personal data." />
          </label>
        </div>
        <div className="inline-toggle">
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={config.guardrails.detect_prompt_injection}
              onChange={(e) => setConfig({
                ...config,
                guardrails: { ...config.guardrails, detect_prompt_injection: e.target.checked },
              })}
            />
            Detect prompt injection <InfoTip text="Scan inputs for attempts to manipulate the AI's behavior through hidden instructions." />
          </label>
        </div>
        <div className="inline-toggle">
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={config.guardrails.block_prompt_injection}
              onChange={(e) => setConfig({
                ...config,
                guardrails: { ...config.guardrails, block_prompt_injection: e.target.checked },
              })}
            />
            Block prompt injection (not just warn) <InfoTip text="Actively block detected prompt injection attempts instead of just warning." />
          </label>
        </div>
        <div className="inline-toggle">
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={config.guardrails.confirm_external_actions}
              onChange={(e) => setConfig({
                ...config,
                guardrails: { ...config.guardrails, confirm_external_actions: e.target.checked },
              })}
            />
            Require confirmation for external actions <InfoTip text="Ask for approval before sending data to external services or APIs." />
          </label>
        </div>

        <div className="inline-toggle">
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={config.guardrails.use_dcg}
              onChange={(e) => setConfig({
                ...config,
                guardrails: { ...config.guardrails, use_dcg: e.target.checked },
              })}
            />
            Destructive Command Guard (DCG) <InfoTip text="Analyzes commands for destructive patterns before execution. Provides an extra layer of protection beyond pattern matching." />
          </label>
        </div>
        <div className="inline-toggle">
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={config.guardrails.dangers_acknowledged}
              onChange={(e) => setConfig({
                ...config,
                guardrails: { ...config.guardrails, dangers_acknowledged: e.target.checked },
              })}
            />
            Acknowledge reduced security <InfoTip text="Required when lowering security level below Standard. Confirms you understand the risks." />
          </label>
        </div>
        {config.guardrails.dangers_acknowledged && (
          <div className="warning-banner">
            You have acknowledged reduced security. Some safety checks may be bypassed.
          </div>
        )}

        <label className="field">
          <span>Default Tool Permission <InfoTip text="How new/unknown tools are handled. RequireConfirmation (default) prompts you before each tool runs. Auto-Approve skips the prompt for trusted tools." /></span>
          <select
            value={config.guardrails.default_tool_permission}
            onChange={(e) => setConfig({
              ...config,
              guardrails: { ...config.guardrails, default_tool_permission: e.target.value },
            })}
          >
            <option value="AutoApprove">Auto-Approve</option>
            <option value="AllowWithLogging">Allow with Logging</option>
            <option value="RequireConfirmation">Require Confirmation</option>
            <option value="Block">Block All</option>
          </select>
        </label>

        <label className="field">
          <span>Dangerous Tool Patterns <InfoTip text="Glob patterns for tool names that should always require extra scrutiny (e.g., execute_*, delete_*)." /></span>
        </label>
        <TagInput
          tags={config.guardrails.dangerous_tool_patterns}
          onChange={(tags) => setConfig({
            ...config,
            guardrails: { ...config.guardrails, dangerous_tool_patterns: tags },
          })}
          placeholder="e.g., execute_*, rm_*, delete_*"
          suggestions={PATTERN_SUGGESTIONS}
        />
      </div>

      {/* Multi-Model Defense */}
      <div className="settings-group">
        <h3>AI Defense Pipeline <InfoTip text="Optional AI-powered safety layers that analyze inputs before they reach Claude. Disabled by default. Enable only if you need prompt injection or content safety detection." /></h3>
        <p className="group-description">
          Optional AI-powered defense layers that detect prompt injection and unsafe content.
          Disabled by default — enable only if needed.
        </p>

        <div className="inline-toggle">
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={config.defense.enabled}
              onChange={(e) => setConfig({
                ...config,
                defense: { ...config.defense, enabled: e.target.checked },
              })}
            />
            Enable AI Defense Pipeline <InfoTip text="Turn on the multi-model defense system. When enabled, messages are scanned by AI models before processing. Requires model downloads on first use." />
          </label>
        </div>

        {config.defense.enabled && (
          <>
            {defenseStatus && (
              <div className="status-indicator">
                <span className={`status-dot ${defenseStatus.deberta_healthy ? 'healthy' : 'unhealthy'}`} />
                <span>
                  {defenseStatus.deberta_healthy ? 'Pipeline active' : 'Pipeline degraded — models loading or unavailable'}
                </span>
                {defenseStatus.deberta_loaded && (
                  <span className="hint"> — DeBERTa loaded</span>
                )}
                {defenseStatus.llama_guard_available && (
                  <span className="hint"> — Llama Guard available</span>
                )}
              </div>
            )}

            <div className="inline-toggle">
              <label className="toggle-label">
                <input
                  type="checkbox"
                  checked={config.defense.fail_open ?? true}
                  onChange={(e) => setConfig({
                    ...config,
                    defense: { ...config.defense, fail_open: e.target.checked },
                  })}
                />
                Allow messages when models are unavailable <InfoTip text="When checked (recommended), messages pass through normally if defense models haven't loaded yet. When unchecked, ALL messages are blocked until models are ready — this can prevent you from using the app entirely." />
              </label>
            </div>
            {!config.defense.fail_open && (
              <div className="warning-banner" style={{ borderColor: 'var(--danger, #e74c3c)' }}>
                All messages will be BLOCKED if defense models fail to load or are unavailable. This can make the app unusable. Only disable this if you are certain the models are working.
              </div>
            )}

            <div className="inline-toggle">
              <label className="toggle-label">
                <input
                  type="checkbox"
                  checked={config.defense.deberta_enabled}
                  onChange={(e) => setConfig({
                    ...config,
                    defense: { ...config.defense, deberta_enabled: e.target.checked },
                  })}
                />
                DeBERTa v3 (prompt injection, &lt;10ms) <InfoTip text="Fast prompt injection detector (<10ms). Uses a fine-tuned DeBERTa model to classify inputs. Downloads ~100MB model on first use." />
              </label>
            </div>
            {config.defense.deberta_enabled && (
              <>
                <label className="field">
                  <span>
                    Detection Threshold <InfoTip text="How sensitive DeBERTa detection is. Lower = more aggressive (more false positives). Higher = more permissive." />
                    <span className="range-value">{config.defense.deberta_threshold.toFixed(2)}</span>
                  </span>
                  <input
                    type="range"
                    min="0" max="1" step="0.01"
                    value={config.defense.deberta_threshold}
                    onChange={(e) => setConfig({
                      ...config,
                      defense: { ...config.defense, deberta_threshold: parseFloat(e.target.value) },
                    })}
                  />
                </label>
                <label className="field">
                  <span>DeBERTa Model Path (optional) <InfoTip text="Path to the DeBERTa ONNX model. Leave empty to auto-download the default model on first use." /></span>
                  <input
                    type="text"
                    value={config.defense.deberta_model_path || ''}
                    onChange={(e) => setConfig({
                      ...config,
                      defense: { ...config.defense, deberta_model_path: e.target.value || undefined },
                    })}
                    placeholder="Auto-download if empty"
                  />
                </label>
              </>
            )}
            <div className="inline-toggle">
              <label className="toggle-label">
                <input
                  type="checkbox"
                  checked={config.defense.llama_guard_enabled}
                  onChange={(e) => setConfig({
                    ...config,
                    defense: { ...config.defense, llama_guard_enabled: e.target.checked },
                  })}
                />
                Llama Guard 3 (content safety) <InfoTip text="Content safety classifier. Detects harmful content categories using Meta's Llama Guard model. Requires Ollama running locally." />
              </label>
            </div>
            {config.defense.llama_guard_enabled && (
              <>
                <label className="field">
                  <span>Llama Guard Mode <InfoTip text="How to run Llama Guard. 'API' connects to an Ollama instance. 'Local' runs the model directly (requires more RAM)." /></span>
                  <select
                    value={config.defense.llama_guard_mode}
                    onChange={(e) => setConfig({
                      ...config,
                      defense: { ...config.defense, llama_guard_mode: e.target.value },
                    })}
                  >
                    <option value="api">API (Ollama)</option>
                    <option value="local">Local</option>
                  </select>
                </label>
                <label className="field">
                  <span>Llama Guard API URL <InfoTip text="URL of the Ollama API server running the Llama Guard model." /></span>
                  <input
                    type="text"
                    value={config.defense.llama_guard_api_url}
                    onChange={(e) => setConfig({
                      ...config,
                      defense: { ...config.defense, llama_guard_api_url: e.target.value },
                    })}
                    placeholder="http://localhost:11434"
                  />
                </label>
                <div className="inline-toggle">
                  <label className="toggle-label">
                    <input
                      type="checkbox"
                      checked={config.defense.allow_remote_llama_guard}
                      onChange={(e) => setConfig({
                        ...config,
                        defense: { ...config.defense, allow_remote_llama_guard: e.target.checked },
                      })}
                    />
                    Allow remote Llama Guard endpoint <InfoTip text="Allow connecting to a Llama Guard endpoint on a remote machine. Disabled by default for security." />
                  </label>
                </div>
                {config.defense.allow_remote_llama_guard && (
                  <div className="warning-banner">
                    Remote endpoints send conversation data over the network. Only use trusted endpoints.
                  </div>
                )}
              </>
            )}
          </>
        )}
      </div>

      {/* Security Audit */}
      <div className="settings-group">
        <h3>Security Audit<InfoTip text="Run a comprehensive security check that inspects your configuration for common misconfigurations, weak settings, and potential vulnerabilities." /></h3>
        <p className="group-description">
          Run an automated security audit to check for misconfigurations and vulnerabilities.
        </p>

        <button className="primary" onClick={handleRunAudit} disabled={runningAudit}>
          {runningAudit ? 'Running Audit...' : 'Run Security Audit'}
        </button>

        {auditReport && (
          <div style={{ marginTop: '12px' }}>
            <div className="info-text" style={{ marginBottom: '8px' }}>
              <strong>{auditReport.passed_count}/{auditReport.total_checks}</strong> checks passed
              {auditReport.findings.length > 0 && (
                <> — <strong>{auditReport.findings.filter(f => f.auto_fixable).length}</strong> auto-fixable</>
              )}
            </div>

            {auditReport.findings.length === 0 ? (
              <div className="info-text" style={{ color: 'var(--success)' }}>All checks passed. Your configuration looks secure.</div>
            ) : (
              <div>
                {auditReport.findings
                  .sort((a, b) => (severityOrder[a.severity] ?? 5) - (severityOrder[b.severity] ?? 5))
                  .map((finding) => (
                    <div key={finding.id} className="mcp-server-card">
                      <div className="mcp-server-header">
                        <span className={`severity-badge severity-${finding.severity.toLowerCase()}`}>
                          {finding.severity}
                        </span>
                        <span className="mcp-server-name">{finding.title}</span>
                      </div>
                      <div style={{ fontSize: '12px', color: 'var(--text-secondary)', margin: '6px 0' }}>
                        {finding.description}
                      </div>
                      {finding.fix_hint && (
                        <div style={{ fontSize: '11px', color: 'var(--text-secondary)', fontStyle: 'italic', margin: '4px 0' }}>
                          Hint: {finding.fix_hint}
                        </div>
                      )}
                      {finding.auto_fixable && (
                        <div className="action-buttons">
                          <button
                            className="primary"
                            disabled={fixingId === finding.id}
                            onClick={() => handleAutoFix(finding.id)}
                          >
                            {fixingId === finding.id ? 'Fixing...' : 'Auto-Fix'}
                          </button>
                        </div>
                      )}
                    </div>
                  ))}
              </div>
            )}
          </div>
        )}
      </div>

      {/* SSRF Protection */}
      <div className="settings-group">
        <h3>SSRF Protection <InfoTip text="Server-Side Request Forgery prevention. Validates all outbound URLs before fetching by checking hostnames and resolved IPs against private/internal network ranges. Fails closed on DNS resolution errors." /></h3>
        <p className="group-description">
          Outbound HTTP requests are validated against private networks, blocked hostnames, and DNS rebinding attacks. These protections are always active and cannot be disabled.
        </p>

        <div className="info-text" style={{ marginBottom: '8px' }}>
          <strong>Protections enabled:</strong>
        </div>
        <ul className="readonly-list">
          <li>Private/internal IP blocking (10.x, 127.x, 172.16-31.x, 192.168.x, 169.254.x, 100.64-127.x)</li>
          <li>IPv6 private range blocking (loopback, link-local, unique-local, embedded IPv4)</li>
          <li>Non-canonical IPv4 literal rejection (octal, hex, short-form, packed decimal)</li>
          <li>DNS rebinding prevention via address pinning</li>
          <li>Cross-origin credential stripping on redirects</li>
          <li>URI scheme restriction (http/https only)</li>
          <li>Fail-closed on DNS resolution errors</li>
        </ul>

        <div className="info-text" style={{ marginTop: '12px', marginBottom: '4px' }}>
          <strong>Blocked hostnames (hardcoded):</strong>
        </div>
        <div className="tag-list readonly">
          {['localhost', 'metadata.google.internal', 'metadata.internal', 'instance-data', '*.localhost', '*.local', '*.internal'].map((h) => (
            <span key={h} className="tag">{h}</span>
          ))}
        </div>

        <div className="info-text" style={{ marginTop: '12px' }}>
          Additional blocked domains can be configured in the <strong>Fetch Security</strong> section below under "Blocked Domains".
        </div>
      </div>

      {/* Env Var Sanitization */}
      <div className="settings-group">
        <h3>Environment Variable Sanitization <InfoTip text="Prevents accidental leakage of API keys, tokens, and passwords when spawning child processes. Hard-blocked variables can never be overridden, even by skills or config." /></h3>
        <p className="group-description">
          Child processes are launched with a sanitized environment. Secret-bearing variables are stripped, and critical system variables are always protected against override attacks.
        </p>

        <div className="info-text" style={{ marginBottom: '4px' }}>
          <strong>Hard-blocked variables (always stripped, prevents injection attacks):</strong>
        </div>
        <div className="tag-list readonly">
          {['PATH', 'HOME', 'USER', 'SHELL', 'LD_PRELOAD', 'LD_LIBRARY_PATH', 'DYLD_INSERT_LIBRARIES', 'DYLD_LIBRARY_PATH', 'DYLD_FRAMEWORK_PATH'].map((v) => (
            <span key={v} className="tag">{v}</span>
          ))}
        </div>

        <div className="info-text" style={{ marginTop: '12px', marginBottom: '4px' }}>
          <strong>Secret patterns blocked (regex-matched variable names):</strong>
        </div>
        <div className="tag-list readonly">
          {['*_API_KEY', '*_TOKEN', '*_PASSWORD', '*_PRIVATE_KEY', '*_SECRET', '*_CREDENTIALS', 'AWS_SECRET_*', 'GH_TOKEN', 'GITHUB_TOKEN'].map((p) => (
            <span key={p} className="tag">{p}</span>
          ))}
        </div>

        <div className="info-text" style={{ marginTop: '12px' }}>
          <strong>Additional protections:</strong> Values containing null bytes are rejected. Values exceeding 32KB trigger warnings. Base64-encoded credential-like values (80+ chars) are flagged.
          In <em>strict mode</em> (used by Docker sandbox), only explicitly allowed variables pass through.
        </div>
      </div>

      {/* Pairing Security */}
      <div className="settings-group">
        <h3>Pairing Security <InfoTip text="Rate limiting and security controls for DM pairing requests across all external channels (Telegram, Discord, WhatsApp, etc.)." /></h3>
        <p className="group-description">
          DM pairing uses cryptographically random 12-character codes (60-bit entropy) with a 15-minute expiry.
          Rate limiting prevents brute-force attacks on pairing codes.
        </p>

        <div className="info-text" style={{ marginBottom: '8px' }}>
          <strong>Rate limits (hardcoded):</strong>
        </div>
        <div className="settings-row">
          <label className="field">
            <span>Max Attempts per Sender</span>
            <input type="number" value={5} disabled />
          </label>
          <label className="field">
            <span>Window</span>
            <input type="text" value="15 minutes" disabled />
          </label>
          <label className="field">
            <span>Lockout Duration</span>
            <input type="text" value="15 minutes" disabled />
          </label>
        </div>

        <div className="info-text" style={{ marginTop: '8px', marginBottom: '8px' }}>
          <strong>Additional limits:</strong>
        </div>
        <ul className="readonly-list">
          <li>Max 20 active pairing requests system-wide</li>
          <li>Max 3 pending requests per sender per channel</li>
          <li>Rate limits are per-sender, per-channel (independent)</li>
          <li>Loopback addresses (127.x, ::1) are exempt from rate limiting</li>
        </ul>

        <div className="info-text" style={{ marginTop: '8px', marginBottom: '8px' }}>
          <strong>Gateway control-plane rate limit:</strong>
        </div>
        <ul className="readonly-list">
          <li>Admin write operations (config.apply, config.patch, update.run): max 3 per minute per client</li>
          <li>Lockout duration: 2 minutes after exceeding limit</li>
        </ul>

        <div className="info-text" style={{ marginTop: '12px' }}>
          Pairing policies (Allowlist vs Pairing mode) can be configured per-channel in the <strong>Channels</strong> tab.
        </div>
      </div>

      {/* Execution Security */}
      <div className="settings-group">
        <h3>Execution Security <InfoTip text="Controls for the command execution tool. Determines what commands the agent can run and with what limits." /></h3>
        <p className="group-description">Configure what commands the agent can execute and safety limits.</p>

        <div className="inline-toggle">
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={config.execute?.enabled ?? false}
              onChange={(e) => setConfig({
                ...config,
                execute: { ...(config.execute ?? { enabled: false, allowed_commands: [], blocked_commands: [], default_timeout_ms: 30000, max_output_bytes: 1048576, use_dcg: true, skill_runtime_exec_enabled: false }), enabled: e.target.checked },
              })}
            />
            Enable command execution <InfoTip text="Master toggle for the execute tool. When disabled, the agent cannot run any commands." />
          </label>
        </div>
        <div className="inline-toggle">
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={config.execute?.use_dcg ?? true}
              onChange={(e) => setConfig({
                ...config,
                execute: { ...config.execute, use_dcg: e.target.checked },
              })}
            />
            Destructive Command Guard <InfoTip text="Analyze commands for destructive patterns before execution." />
          </label>
        </div>
        <div className="inline-toggle">
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={config.execute?.skill_runtime_exec_enabled ?? false}
              onChange={(e) => setConfig({
                ...config,
                execute: { ...config.execute, skill_runtime_exec_enabled: e.target.checked },
              })}
            />
            Allow skills to trigger execution <InfoTip text="When enabled, skills can invoke the execute tool at runtime. When disabled, skill-initiated execution is blocked." />
          </label>
        </div>

        <label className="field">
          <span>Default Timeout (ms) <InfoTip text="Maximum time a command can run before being killed." /></span>
          <input
            type="number"
            min={1000}
            value={config.execute?.default_timeout_ms ?? 30000}
            onChange={(e) => setConfig({
              ...config,
              execute: { ...config.execute, default_timeout_ms: parseInt(e.target.value) || 30000 },
            })}
          />
        </label>
        <label className="field">
          <span>Max Output Bytes <InfoTip text="Maximum size of command output captured. Larger output is truncated." /></span>
          <input
            type="number"
            min={1024}
            value={config.execute?.max_output_bytes ?? 1048576}
            onChange={(e) => setConfig({
              ...config,
              execute: { ...config.execute, max_output_bytes: parseInt(e.target.value) || 1048576 },
            })}
          />
        </label>
        <label className="field">
          <span>Working Directory (optional) <InfoTip text="Default working directory for commands. Leave empty to use the app's directory." /></span>
          <input
            type="text"
            value={config.execute?.working_directory ?? ''}
            placeholder="Leave empty for default"
            onChange={(e) => setConfig({
              ...config,
              execute: { ...config.execute, working_directory: e.target.value || undefined },
            })}
          />
        </label>

        <label className="field">
          <span>Allowed Commands <InfoTip text="Commands the agent is allowed to run. Empty means all commands are allowed (when execution is enabled)." /></span>
        </label>
        <TagInput
          tags={config.execute?.allowed_commands ?? []}
          onChange={(tags) => setConfig({ ...config, execute: { ...config.execute, allowed_commands: tags } })}
          placeholder="e.g., ls, cat, grep"
          suggestions={SAFE_COMMAND_SUGGESTIONS}
        />

        <label className="field">
          <span>Blocked Commands <InfoTip text="Commands that are always blocked, even if execution is enabled." /></span>
        </label>
        <TagInput
          tags={config.execute?.blocked_commands ?? []}
          onChange={(tags) => setConfig({ ...config, execute: { ...config.execute, blocked_commands: tags } })}
          placeholder="e.g., rm, shutdown, reboot"
          suggestions={DANGEROUS_COMMAND_SUGGESTIONS}
        />
      </div>

      {/* Filesystem Security */}
      <div className="settings-group">
        <h3>Filesystem Security <InfoTip text="Controls which paths the agent can read from and write to, and size limits for file operations." /></h3>
        <p className="group-description">Configure filesystem access boundaries and size limits.</p>

        <label className="field">
          <span>Allowed Paths <InfoTip text="Paths the agent can access. Empty defaults to the home directory." /></span>
        </label>
        <TagInput
          tags={config.filesystem?.allowed_paths ?? []}
          onChange={(tags) => setConfig({ ...config, filesystem: { ...config.filesystem, allowed_paths: tags } })}
          placeholder="e.g., ~/projects"
          suggestions={[...SAFE_PATH_SUGGESTIONS, ...SENSITIVE_PATH_SUGGESTIONS]}
        />

        <label className="field">
          <span>Blocked Paths <InfoTip text="Paths that are always blocked from access." /></span>
        </label>
        <TagInput
          tags={config.filesystem?.blocked_paths ?? []}
          onChange={(tags) => setConfig({ ...config, filesystem: { ...config.filesystem, blocked_paths: tags } })}
          placeholder="e.g., sensitive system paths"
          suggestions={SENSITIVE_PATH_SUGGESTIONS}
        />

        <label className="field">
          <span>Max Read Bytes <InfoTip text="Maximum file size the agent can read. Default: 1MB." /></span>
          <input
            type="number"
            min={1024}
            value={config.filesystem?.max_read_bytes ?? 1048576}
            onChange={(e) => setConfig({
              ...config,
              filesystem: { ...config.filesystem, max_read_bytes: parseInt(e.target.value) || 1048576 },
            })}
          />
        </label>
        <label className="field">
          <span>Max Write Bytes <InfoTip text="Maximum file size the agent can write. Default: 10MB." /></span>
          <input
            type="number"
            min={1024}
            value={config.filesystem?.max_write_bytes ?? 10485760}
            onChange={(e) => setConfig({
              ...config,
              filesystem: { ...config.filesystem, max_write_bytes: parseInt(e.target.value) || 10485760 },
            })}
          />
        </label>
      </div>

      {/* Fetch Security */}
      <div className="settings-group">
        <h3>Fetch Security <InfoTip text="Controls which domains the agent can fetch data from and response size limits." /></h3>
        <p className="group-description">Configure network fetch boundaries and limits.</p>

        <label className="field">
          <span>Allowed Domains <InfoTip text="Domains the agent can fetch from. Empty means all external domains are allowed." /></span>
        </label>
        <TagInput
          tags={config.fetch?.allowed_domains ?? []}
          onChange={(tags) => setConfig({ ...config, fetch: { ...config.fetch, allowed_domains: tags } })}
          placeholder="e.g., api.github.com"
          suggestions={ALLOWED_DOMAIN_SUGGESTIONS}
        />

        <label className="field">
          <span>Blocked Domains <InfoTip text="Domains that are always blocked from fetching." /></span>
        </label>
        <TagInput
          tags={config.fetch?.blocked_domains ?? []}
          onChange={(tags) => setConfig({ ...config, fetch: { ...config.fetch, blocked_domains: tags } })}
          placeholder="e.g., evil.com"
          suggestions={BLOCKED_DOMAIN_SUGGESTIONS}
        />

        <label className="field">
          <span>Max Response Bytes <InfoTip text="Maximum response body size the agent will accept. Default: 1MB." /></span>
          <input
            type="number"
            min={1024}
            value={config.fetch?.max_response_bytes ?? 1048576}
            onChange={(e) => setConfig({
              ...config,
              fetch: { ...config.fetch, max_response_bytes: parseInt(e.target.value) || 1048576 },
            })}
          />
        </label>
        <label className="field">
          <span>Default Timeout (ms) <InfoTip text="Maximum time in milliseconds to wait for a fetch response. Default: 30000 (30 seconds)." /></span>
          <input
            type="number"
            min={1000}
            value={config.fetch?.default_timeout_ms ?? 30000}
            onChange={(e) => setConfig({
              ...config,
              fetch: { ...config.fetch, default_timeout_ms: parseInt(e.target.value) || 30000 },
            })}
          />
        </label>
      </div>

      {/* Webhook Security */}
      <div className="settings-group">
        <h3>Webhook Security <InfoTip text="Controls for the webhook server that receives incoming messages from WhatsApp, Slack, and other channel integrations." /></h3>
        <p className="group-description">Configure the webhook server's security settings, TLS, and rate limiting.</p>

        <div className="inline-toggle">
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={config.webhooks?.enabled ?? false}
              onChange={(e) => setConfig({
                ...config,
                webhooks: { ...config.webhooks, enabled: e.target.checked },
              })}
            />
            Enable Webhooks <InfoTip text="Master toggle for generic /webhook/* endpoints. Channel-specific routes (Slack, WhatsApp, Teams, etc.) are controlled by each channel's own enabled setting." />
          </label>
        </div>

        <label className="field">
          <span>Port <InfoTip text="TCP port the webhook server listens on. Default: 18791." /></span>
          <input
            type="number"
            min={1024} max={65535}
            value={config.webhooks?.port ?? 18791}
            onChange={(e) => setConfig({
              ...config,
              webhooks: { ...config.webhooks, port: parseInt(e.target.value) || 18791 },
            })}
          />
        </label>

        <label className="field">
          <span>Auth Token <InfoTip text="Required bearer token for generic /webhook/* requests. Include as Authorization: Bearer &lt;token&gt;." /></span>
          <input
            type="password"
            value={config.webhooks?.auth_token ?? ''}
            placeholder="Required bearer token"
            onChange={(e) => setConfig({
              ...config,
              webhooks: { ...config.webhooks, auth_token: e.target.value || undefined },
            })}
          />
        </label>

        <h4 style={{ margin: '12px 0 4px' }}>TLS</h4>
        <div className="inline-toggle">
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={config.webhooks?.tls?.enabled ?? false}
              onChange={(e) => setConfig({
                ...config,
                webhooks: { ...config.webhooks, tls: { ...(config.webhooks?.tls ?? { enabled: false, auto_generate: false }), enabled: e.target.checked } },
              })}
            />
            Enable TLS <InfoTip text="Serve webhooks over HTTPS. Required for production WhatsApp integration." />
          </label>
        </div>
        {config.webhooks?.tls?.enabled && (
          <>
            <div className="inline-toggle">
              <label className="toggle-label">
                <input
                  type="checkbox"
                  checked={config.webhooks?.tls?.auto_generate ?? false}
                  onChange={(e) => setConfig({
                    ...config,
                    webhooks: { ...config.webhooks, tls: { ...config.webhooks.tls, auto_generate: e.target.checked } },
                  })}
                />
                Auto-Generate Certificates <InfoTip text="Automatically generate self-signed TLS certificates. For production, use proper certificates." />
              </label>
            </div>
            <label className="field">
              <span>Certificate Path <InfoTip text="Path to the TLS certificate file (.pem or .crt)." /></span>
              <input
                type="text"
                value={config.webhooks?.tls?.cert_path ?? ''}
                placeholder="Path to cert.pem"
                onChange={(e) => setConfig({
                  ...config,
                  webhooks: { ...config.webhooks, tls: { ...config.webhooks.tls, cert_path: e.target.value || undefined } },
                })}
              />
            </label>
            <label className="field">
              <span>Key Path <InfoTip text="Path to the TLS private key file (.pem or .key)." /></span>
              <input
                type="text"
                value={config.webhooks?.tls?.key_path ?? ''}
                placeholder="Path to key.pem"
                onChange={(e) => setConfig({
                  ...config,
                  webhooks: { ...config.webhooks, tls: { ...config.webhooks.tls, key_path: e.target.value || undefined } },
                })}
              />
            </label>
          </>
        )}

        <h4 style={{ margin: '12px 0 4px' }}>Rate Limit</h4>
        <div className="settings-row">
          <label className="field">
            <span>Max Attempts <InfoTip text="Maximum number of requests allowed within the time window before rate limiting kicks in." /></span>
            <input
              type="number"
              min={1}
              value={config.webhooks?.rate_limit?.max_attempts ?? 10}
              onChange={(e) => setConfig({
                ...config,
                webhooks: { ...config.webhooks, rate_limit: { ...(config.webhooks?.rate_limit ?? { max_attempts: 10, window_secs: 60, lockout_secs: 300 }), max_attempts: parseInt(e.target.value) || 10 } },
              })}
            />
          </label>
          <label className="field">
            <span>Window (seconds) <InfoTip text="Time window in seconds for rate limit counting." /></span>
            <input
              type="number"
              min={1}
              value={config.webhooks?.rate_limit?.window_secs ?? 60}
              onChange={(e) => setConfig({
                ...config,
                webhooks: { ...config.webhooks, rate_limit: { ...(config.webhooks?.rate_limit ?? { max_attempts: 10, window_secs: 60, lockout_secs: 300 }), window_secs: parseInt(e.target.value) || 60 } },
              })}
            />
          </label>
          <label className="field">
            <span>Lockout (seconds) <InfoTip text="Duration in seconds a client is locked out after exceeding the rate limit." /></span>
            <input
              type="number"
              min={1}
              value={config.webhooks?.rate_limit?.lockout_secs ?? 300}
              onChange={(e) => setConfig({
                ...config,
                webhooks: { ...config.webhooks, rate_limit: { ...(config.webhooks?.rate_limit ?? { max_attempts: 10, window_secs: 60, lockout_secs: 300 }), lockout_secs: parseInt(e.target.value) || 300 } },
              })}
            />
          </label>
        </div>
      </div>

      {/* Tool Permissions */}
      <div className="settings-group">
        <h3>Tool Permissions<InfoTip text="Fine-grained permission controls for each MCP server and its tools. Override the default permission level per server or per tool." /></h3>
        <p className="group-description">
          Configure per-server default permissions and per-tool overrides.
        </p>

        {Object.keys(toolPermissions).length === 0 && mcpServers.length === 0 && (
          <div className="info-text">No MCP servers configured. Add servers in the Tools tab to manage permissions.</div>
        )}

        {/* Configured servers */}
        {Object.entries(toolPermissions).map(([serverName, perms]) => (
          <div key={serverName} className="mcp-server-card">
            <div className="mcp-server-header">
              <span className="mcp-server-name">{serverName}</span>
              <span className="mcp-tool-count">
                {Object.keys(perms.tool_overrides).length} override{Object.keys(perms.tool_overrides).length !== 1 ? 's' : ''}
              </span>
            </div>
            <label className="field">
              <span>Default Permission</span>
              <select
                value={perms.default_permission}
                disabled={savingPermServer === serverName}
                onChange={async (e) => {
                  setSavingPermServer(serverName);
                  try {
                    const updated = {
                      ...toolPermissions,
                      [serverName]: { ...perms, default_permission: e.target.value },
                    };
                    await invoke('update_tool_permissions', { permissions: updated });
                    await loadToolPermissions();
                  } catch (error) {
                    notifyError('Security', `Failed to update permissions: ${error}`);
                  } finally {
                    setSavingPermServer(null);
                  }
                }}
              >
                <option value="AutoApprove">Auto-Approve</option>
                <option value="AllowWithLogging">Allow with Logging</option>
                <option value="RequireConfirmation">Require Confirmation</option>
                <option value="Block">Block</option>
              </select>
            </label>
            {Object.keys(perms.tool_overrides).length > 0 && (
              <div style={{ fontSize: '12px', color: 'var(--text-secondary)', marginTop: '4px' }}>
                <strong>Tool overrides:</strong>
                <ul style={{ margin: '4px 0 0 16px', padding: 0 }}>
                  {Object.entries(perms.tool_overrides).map(([tool, perm]) => (
                    <li key={tool}>{tool}: {perm}</li>
                  ))}
                </ul>
              </div>
            )}
          </div>
        ))}

        {/* Unconfigured servers */}
        {mcpServers
          .filter(s => !toolPermissions[s.name])
          .map((server) => (
            <div key={server.name} className="mcp-server-card">
              <div className="mcp-server-header">
                <span className="mcp-server-name">{server.name}</span>
                <span className="mcp-server-command">No custom permissions</span>
              </div>
              <div className="action-buttons">
                <button onClick={async () => {
                  setSavingPermServer(server.name);
                  try {
                    const updated = {
                      ...toolPermissions,
                      [server.name]: { default_permission: 'RequireConfirmation', tool_overrides: {} },
                    };
                    await invoke('update_tool_permissions', { permissions: updated });
                    await loadToolPermissions();
                  } catch (error) {
                    notifyError('Security', `Failed to set permissions: ${error}`);
                  } finally {
                    setSavingPermServer(null);
                  }
                }} disabled={savingPermServer === server.name}>
                  {savingPermServer === server.name ? 'Loading...' : 'Set Custom Permissions'}
                </button>
              </div>
            </div>
          ))}
      </div>
    </div>
  );
}
