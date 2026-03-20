/**
 * UsageWidget — mini credit usage widget for the NexiBot Dashboard.
 *
 * Fetches GET /api/usage/quota and GET /api/usage/breakdown from the KN
 * REST API (via kn_base_url stored in config), displays:
 *   • A horizontal progress bar (used / allocation)
 *   • Current balance and tier
 *   • Per-operation bar chart for the current month
 *
 * The widget is shown on the NexiBot Settings > System panel and can also be
 * embedded in the Dashboard section.  If the KN API is unreachable (user not
 * signed in, offline, etc.) it renders a soft "not connected" state and does
 * NOT throw or log spam.
 */

import { useState, useEffect, useCallback } from 'react';
import './UsageWidget.css';

// ── Types ──────────────────────────────────────────────────────────────────────

interface QuotaData {
  user_id: string;
  current_balance: number;
  monthly_allocation: number;
  overage_credits: number;
  purchased_credits: number;
  period_start: string;
  period_end: string;
  tier: string;
  used_credits: number;
  used_fraction: number;
}

interface BreakdownItem {
  operation: string;
  service_name: string | null;
  credits_used: number;
  call_count: number;
  date: string;
}

interface BreakdownData {
  user_id: string;
  start: string;
  end: string;
  total_credits_used: number;
  items: BreakdownItem[];
}

interface AggregatedOp {
  operation: string;
  credits: number;
  calls: number;
}

// ── Helpers ────────────────────────────────────────────────────────────────────

function opLabel(op: string): string {
  switch (op) {
    case 'api_call': return 'API calls';
    case 'chat': return 'Chat';
    case 'llm_generate': return 'LLM generation';
    case 'vector_search': return 'Vector search';
    case 'web_fetch': return 'Web fetch';
    case 'research': return 'Research';
    case 'sync': return 'Sync';
    default: return op.replace(/_/g, ' ');
  }
}

function tierColor(tier: string): string {
  switch (tier) {
    case 'free': return '#6c7086';
    case 'personal': return '#89b4fa';
    case 'pro': return '#a6e3a1';
    case 'family': return '#fab387';
    case 'team':
    case 'business':
    case 'enterprise': return '#cba6f7';
    default: return '#89b4fa';
  }
}

function barColor(fraction: number): string {
  if (fraction >= 0.95) return '#f38ba8';
  if (fraction >= 0.80) return '#fab387';
  return '#a6e3a1';
}

// ── Component ──────────────────────────────────────────────────────────────────

interface Props {
  /** KN REST API base URL (from config.k2k.kn_base_url or env) */
  knBaseUrl?: string;
  /** KN auth token (from config.k2k.kn_auth_token) */
  authToken?: string;
}

export function UsageWidget({ knBaseUrl, authToken }: Props) {
  const [quota, setQuota] = useState<QuotaData | null>(null);
  const [breakdown, setBreakdown] = useState<BreakdownData | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const base = (knBaseUrl || '').replace(/\/$/, '');

  const fetchData = useCallback(async () => {
    if (!authToken) {
      setError('not_authenticated');
      setLoading(false);
      return;
    }

    setLoading(true);
    setError(null);

    try {
      const headers: HeadersInit = {
        Authorization: `Bearer ${authToken}`,
        'Content-Type': 'application/json',
      };

      const [quotaRes, breakdownRes] = await Promise.all([
        fetch(`${base}/api/usage/quota`, { headers }),
        fetch(`${base}/api/usage/breakdown`, { headers }),
      ]);

      if (!quotaRes.ok) throw new Error(`quota ${quotaRes.status}`);
      if (!breakdownRes.ok) throw new Error(`breakdown ${breakdownRes.status}`);

      const [q, b] = await Promise.all([quotaRes.json(), breakdownRes.json()]);
      setQuota(q as QuotaData);
      setBreakdown(b as BreakdownData);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, [base, authToken]);

  useEffect(() => {
    fetchData();
  }, [fetchData]);

  // ── Render: not authenticated ──────────────────────────────────────────────

  if (!authToken) {
    return (
      <div className="uw-container uw-placeholder">
        <span className="uw-placeholder-icon">📊</span>
        <p className="uw-placeholder-text">Sign in to Knowledge Nexus to see your credit usage.</p>
      </div>
    );
  }

  // ── Render: loading ────────────────────────────────────────────────────────

  if (loading) {
    return (
      <div className="uw-container uw-loading">
        <div className="uw-spinner" aria-hidden="true" />
        <span>Loading usage…</span>
      </div>
    );
  }

  // ── Render: error ──────────────────────────────────────────────────────────

  if (error) {
    return (
      <div className="uw-container uw-error">
        <p className="uw-error-text">Could not load usage data.</p>
        <button className="uw-retry-btn" onClick={fetchData}>Retry</button>
      </div>
    );
  }

  if (!quota) return null;

  // ── Aggregate breakdown by operation ──────────────────────────────────────

  const aggMap = new Map<string, AggregatedOp>();
  for (const item of (breakdown?.items ?? [])) {
    const key = item.operation;
    const existing = aggMap.get(key) ?? { operation: key, credits: 0, calls: 0 };
    existing.credits += item.credits_used;
    existing.calls += item.call_count;
    aggMap.set(key, existing);
  }
  const aggregated = Array.from(aggMap.values()).sort((a, b) => b.credits - a.credits);
  const maxCredits = aggregated[0]?.credits || 1;

  const usedFraction = Math.min(1, quota.used_fraction);
  const allocation =
    quota.monthly_allocation + quota.overage_credits + quota.purchased_credits;

  // ── Render: widget ─────────────────────────────────────────────────────────

  return (
    <div className="uw-container">
      {/* Header */}
      <div className="uw-header">
        <span className="uw-title">Credits</span>
        <span
          className="uw-tier-badge"
          style={{ background: tierColor(quota.tier) }}
        >
          {quota.tier}
        </span>
        <button className="uw-refresh-btn" onClick={fetchData} aria-label="Refresh usage">⟳</button>
      </div>

      {/* Progress bar */}
      <div className="uw-progress-track" role="progressbar" aria-valuenow={Math.round(usedFraction * 100)} aria-valuemin={0} aria-valuemax={100}>
        <div
          className="uw-progress-fill"
          style={{
            width: `${usedFraction * 100}%`,
            background: barColor(usedFraction),
          }}
        />
      </div>

      {/* Balance label */}
      <div className="uw-balance-row">
        <span className="uw-balance-used">
          {quota.used_credits.toLocaleString()} used
        </span>
        <span className="uw-balance-total">
          {allocation.toLocaleString()} total
        </span>
      </div>

      {/* Warning */}
      {usedFraction >= 0.8 && (
        <div className={`uw-warning ${usedFraction >= 0.95 ? 'uw-warning-critical' : ''}`}>
          {usedFraction >= 1
            ? 'Credits exhausted — upgrade to continue.'
            : `${Math.round(usedFraction * 100)}% used — ${quota.current_balance.toLocaleString()} remaining.`}
        </div>
      )}

      {/* Period */}
      <div className="uw-period">
        Period: {new Date(quota.period_start).toLocaleDateString()} –{' '}
        {new Date(quota.period_end).toLocaleDateString()}
      </div>

      {/* Breakdown bar chart */}
      {aggregated.length > 0 && (
        <div className="uw-breakdown">
          <div className="uw-breakdown-title">By operation (this month)</div>
          {aggregated.slice(0, 6).map((op) => (
            <div key={op.operation} className="uw-breakdown-row">
              <span className="uw-op-label">{opLabel(op.operation)}</span>
              <div className="uw-op-track">
                <div
                  className="uw-op-fill"
                  style={{ width: `${(op.credits / maxCredits) * 100}%` }}
                />
              </div>
              <span className="uw-op-value">{op.credits.toLocaleString()}</span>
            </div>
          ))}
          {breakdown && (
            <div className="uw-breakdown-total">
              Total: {breakdown.total_credits_used.toLocaleString()} credits
            </div>
          )}
        </div>
      )}
    </div>
  );
}
