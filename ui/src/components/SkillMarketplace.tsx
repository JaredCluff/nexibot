/**
 * SkillMarketplace — Browse, preview, and install skills from ClawHub.
 *
 * Uses Tauri commands:
 *   search_clawhub(query, limit)        → SkillSummary[]
 *   get_clawhub_skill_info(slug)        → SkillInfoResponse (includes security report)
 *   install_clawhub_skill(slug, force)  → InstallResult
 *
 * UX:
 *   1. Search bar → results grid of skill cards
 *   2. Click a card → detail panel with description, security badge, install button
 *   3. Install → shows security report, confirms, installs
 *   4. Installed skills show a green "Installed" badge and no install button
 */

import { useState, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import './SkillMarketplace.css';

// ── Types ──────────────────────────────────────────────────────────────────────

interface SkillSummary {
  slug: string;
  name: string;
  description: string;
  author: string;
  version: string;
  downloads: number;
  rating: number;
  tags: string[];
}

interface SecurityFinding {
  severity: string;
  category: string;
  description: string;
}

interface SecurityReport {
  skill_name: string;
  risk_level: string;   // "Safe" | "Caution" | "Warning" | "Dangerous"
  overall_score: number; // 0.0 (dangerous) – 1.0 (safe)
  findings: SecurityFinding[];
  summary: string;
}

interface SkillInfoResponse extends SkillSummary {
  security_report: SecurityReport;
}

interface InstallResult {
  success: boolean;
  skill_id: string;
  security_report: SecurityReport;
  message: string;
}

// ── Helpers ────────────────────────────────────────────────────────────────────

function riskColor(level: string): string {
  switch (level.toLowerCase()) {
    case 'safe':     return '#a6e3a1';
    case 'low':      return '#a6e3a1';
    case 'medium':   return '#f9e2af';
    case 'high':     return '#fab387';
    case 'critical': return '#f38ba8';
    default:         return '#6c7086';
  }
}

function riskIcon(level: string): string {
  switch (level.toLowerCase()) {
    case 'safe':
    case 'low':     return '✓';
    case 'medium':  return '⚠';
    case 'high':
    case 'critical': return '✕';
    default:        return '?';
  }
}

function StarRating({ rating }: { rating: number }) {
  const full = Math.floor(rating);
  const half = rating - full >= 0.5;
  return (
    <span className="sm-stars" aria-label={`${rating.toFixed(1)} stars`}>
      {Array.from({ length: 5 }, (_, i) => (
        <span key={i} className={i < full ? 'sm-star filled' : (i === full && half ? 'sm-star half' : 'sm-star')}>
          ★
        </span>
      ))}
      <span className="sm-rating-num">{rating.toFixed(1)}</span>
    </span>
  );
}

// ── Skill card ─────────────────────────────────────────────────────────────────

interface SkillCardProps {
  skill: SkillSummary;
  installedSlugs: Set<string>;
  onSelect: (skill: SkillSummary) => void;
}

function SkillCard({ skill, installedSlugs, onSelect }: SkillCardProps) {
  const installed = installedSlugs.has(skill.slug);
  return (
    <div className="sm-card" onClick={() => onSelect(skill)} tabIndex={0} role="button"
         onKeyDown={e => e.key === 'Enter' && onSelect(skill)}>
      <div className="sm-card-header">
        <span className="sm-card-name">{skill.name}</span>
        {installed && <span className="sm-installed-badge">Installed</span>}
      </div>
      <p className="sm-card-desc">{skill.description}</p>
      <div className="sm-card-meta">
        <span className="sm-card-author">by {skill.author}</span>
        <StarRating rating={skill.rating} />
        <span className="sm-card-downloads">{skill.downloads.toLocaleString()} ↓</span>
      </div>
      <div className="sm-card-tags">
        {skill.tags.slice(0, 4).map(tag => (
          <span key={tag} className="sm-tag">{tag}</span>
        ))}
      </div>
    </div>
  );
}

// ── Detail panel ───────────────────────────────────────────────────────────────

interface DetailPanelProps {
  skill: SkillSummary;
  installedSlugs: Set<string>;
  onInstall: (slug: string) => void;
  onClose: () => void;
  installing: boolean;
}

function DetailPanel({ skill, installedSlugs, onInstall, onClose, installing }: DetailPanelProps) {
  const [info, setInfo] = useState<SkillInfoResponse | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [loadingInfo, setLoadingInfo] = useState(true);
  const installed = installedSlugs.has(skill.slug);

  useEffect(() => {
    setLoadingInfo(true);
    setLoadError(null);
    invoke<SkillInfoResponse>('get_clawhub_skill_info', { slug: skill.slug })
      .then(setInfo)
      .catch(e => setLoadError(String(e)))
      .finally(() => setLoadingInfo(false));
  }, [skill.slug]);

  const report = info?.security_report;

  return (
    <div className="sm-detail-overlay" onClick={onClose}>
      <div className="sm-detail-panel" onClick={e => e.stopPropagation()}>
        {/* Header */}
        <div className="sm-detail-header">
          <div>
            <h2 className="sm-detail-title">{skill.name}</h2>
            <span className="sm-detail-author">by {skill.author}  ·  v{skill.version}</span>
          </div>
          <button className="sm-close-btn" onClick={onClose} aria-label="Close">✕</button>
        </div>

        <p className="sm-detail-desc">{skill.description}</p>

        <div className="sm-detail-meta">
          <StarRating rating={skill.rating} />
          <span className="sm-card-downloads">{skill.downloads.toLocaleString()} downloads</span>
          <div className="sm-card-tags">
            {skill.tags.map(tag => <span key={tag} className="sm-tag">{tag}</span>)}
          </div>
        </div>

        {/* Security section */}
        <div className="sm-security-section">
          <h3 className="sm-section-title">Security Analysis</h3>
          {loadingInfo && <div className="sm-loading-text">Analysing…</div>}
          {loadError && <div className="sm-error-text">Security scan unavailable: {loadError}</div>}
          {report && (
            <>
              <div className="sm-risk-badge" style={{ color: riskColor(report.risk_level) }}>
                <span className="sm-risk-icon">{riskIcon(report.risk_level)}</span>
                {report.risk_level} risk  ·  score {(report.overall_score * 100).toFixed(0)}/100
              </div>
              <p className="sm-risk-summary">{report.summary}</p>
              {report.findings.length > 0 && (
                <ul className="sm-findings">
                  {report.findings.map((f, i) => (
                    <li key={i} className="sm-finding">
                      <span className="sm-finding-sev" style={{ color: riskColor(f.severity) }}>
                        [{f.severity}]
                      </span>{' '}
                      {f.category}: {f.description}
                    </li>
                  ))}
                </ul>
              )}
            </>
          )}
        </div>

        {/* Action */}
        <div className="sm-detail-actions">
          {installed
            ? <span className="sm-installed-label">✓ Installed</span>
            : (
              <button
                className="sm-install-btn"
                onClick={() => onInstall(skill.slug)}
                disabled={installing || loadingInfo}
              >
                {installing ? 'Installing…' : 'Install Skill'}
              </button>
            )
          }
        </div>
      </div>
    </div>
  );
}

// ── Install result toast ───────────────────────────────────────────────────────

function ResultToast({ result, onDismiss }: { result: InstallResult; onDismiss: () => void }) {
  useEffect(() => {
    const t = setTimeout(onDismiss, 5000);
    return () => clearTimeout(t);
  }, [onDismiss]);

  return (
    <div className={`sm-toast ${result.success ? 'sm-toast-ok' : 'sm-toast-err'}`}>
      {result.success
        ? `✓ ${result.skill_id} installed successfully`
        : `✕ Install failed: ${result.message}`}
      <button className="sm-toast-dismiss" onClick={onDismiss}>✕</button>
    </div>
  );
}

// ── Main component ─────────────────────────────────────────────────────────────

interface Props {
  installedSlugs?: Set<string>;
  onInstalled?: (slug: string) => void;
}

export function SkillMarketplace({ installedSlugs = new Set(), onInstalled }: Props) {
  const [query, setQuery] = useState('');
  const [results, setResults] = useState<SkillSummary[]>([]);
  const [loading, setLoading] = useState(false);
  const [searchError, setSearchError] = useState<string | null>(null);
  const [selected, setSelected] = useState<SkillSummary | null>(null);
  const [installing, setInstalling] = useState(false);
  const [installResult, setInstallResult] = useState<InstallResult | null>(null);
  const [localInstalled, setLocalInstalled] = useState<Set<string>>(installedSlugs);

  // Initial featured load
  useEffect(() => {
    handleSearch('');
  }, []);

  const handleSearch = useCallback(async (q: string) => {
    setLoading(true);
    setSearchError(null);
    try {
      const res = await invoke<SkillSummary[]>('search_clawhub', {
        query: q,
        limit: 24,
      });
      setResults(res);
    } catch (e) {
      setSearchError(String(e));
      setResults([]);
    } finally {
      setLoading(false);
    }
  }, []);

  const handleInstall = useCallback(async (slug: string) => {
    setInstalling(true);
    try {
      const result = await invoke<InstallResult>('install_clawhub_skill', {
        slug,
        forceInstall: false,
      });
      setInstallResult(result);
      if (result.success) {
        setLocalInstalled(prev => new Set([...prev, slug]));
        onInstalled?.(slug);
        setSelected(null);
      }
    } catch (e) {
      setInstallResult({
        success: false,
        skill_id: slug,
        security_report: {} as SecurityReport,
        message: String(e),
      });
    } finally {
      setInstalling(false);
    }
  }, [onInstalled]);

  return (
    <div className="sm-container">
      {/* Toast */}
      {installResult && (
        <ResultToast result={installResult} onDismiss={() => setInstallResult(null)} />
      )}

      {/* Search bar */}
      <div className="sm-search-row">
        <input
          className="sm-search-input"
          type="search"
          placeholder="Search skills…"
          value={query}
          onChange={e => setQuery(e.target.value)}
          onKeyDown={e => e.key === 'Enter' && handleSearch(query)}
        />
        <button className="sm-search-btn" onClick={() => handleSearch(query)}>Search</button>
      </div>

      {/* Results */}
      {loading && (
        <div className="sm-status">
          <div className="sm-spinner" aria-hidden="true" />
          <span>Loading skills…</span>
        </div>
      )}

      {searchError && (
        <div className="sm-status sm-status-error">
          Could not load skills: {searchError}
          <button className="sm-retry-btn" onClick={() => handleSearch(query)}>Retry</button>
        </div>
      )}

      {!loading && !searchError && results.length === 0 && (
        <div className="sm-status">No skills found{query ? ` for "${query}"` : ''}.</div>
      )}

      {!loading && results.length > 0 && (
        <div className="sm-grid">
          {results.map(skill => (
            <SkillCard
              key={skill.slug}
              skill={skill}
              installedSlugs={localInstalled}
              onSelect={setSelected}
            />
          ))}
        </div>
      )}

      {/* Detail panel */}
      {selected && (
        <DetailPanel
          skill={selected}
          installedSlugs={localInstalled}
          onInstall={handleInstall}
          onClose={() => setSelected(null)}
          installing={installing}
        />
      )}
    </div>
  );
}
