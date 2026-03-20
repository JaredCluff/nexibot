//! CLI-compatible security audit runner.
//!
//! Provides a function that runs the security audit and formats output
//! for CLI consumption (JSON or human-readable with ANSI colours).
#![allow(dead_code)]

use serde_json::json;
use tracing::info;

use crate::security::audit::{self, SecurityAuditReport, SecurityAuditSeverity};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Output format for the audit report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditOutputFormat {
    /// Pretty-printed JSON.
    Json,
    /// Colourised human-readable text (ANSI escape codes).
    HumanReadable,
}

// ---------------------------------------------------------------------------
// ANSI colour helpers
// ---------------------------------------------------------------------------

const ANSI_RESET: &str = "\x1b[0m";
const ANSI_BOLD: &str = "\x1b[1m";
const ANSI_RED: &str = "\x1b[31m";
const ANSI_YELLOW: &str = "\x1b[33m";
const ANSI_BLUE: &str = "\x1b[34m";
const ANSI_GREEN: &str = "\x1b[32m";
const ANSI_CYAN: &str = "\x1b[36m";
const ANSI_DIM: &str = "\x1b[2m";

fn severity_color(severity: &SecurityAuditSeverity) -> &'static str {
    match severity {
        SecurityAuditSeverity::Critical => ANSI_RED,
        SecurityAuditSeverity::High => ANSI_YELLOW,
        SecurityAuditSeverity::Medium => ANSI_BLUE,
        SecurityAuditSeverity::Low => ANSI_CYAN,
        SecurityAuditSeverity::Info => ANSI_DIM,
    }
}

fn severity_icon(severity: &SecurityAuditSeverity) -> &'static str {
    match severity {
        SecurityAuditSeverity::Critical => "[!!]",
        SecurityAuditSeverity::High => "[! ]",
        SecurityAuditSeverity::Medium => "[* ]",
        SecurityAuditSeverity::Low => "[- ]",
        SecurityAuditSeverity::Info => "[i ]",
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Run the security audit and format the output.
///
/// Returns `(output_string, exit_code)` where:
/// - `exit_code = 0` : no critical or high findings
/// - `exit_code = 1` : at least one high-severity finding
/// - `exit_code = 2` : at least one critical-severity finding
pub fn run_cli_audit(
    config: &crate::config::NexiBotConfig,
    format: AuditOutputFormat,
) -> (String, i32) {
    info!("[CLI_AUDIT] Running security audit (format: {:?})", format);

    let report = audit::run_full_audit(config);

    let has_critical = report
        .findings
        .iter()
        .any(|f| f.severity == SecurityAuditSeverity::Critical);
    let has_high = report
        .findings
        .iter()
        .any(|f| f.severity == SecurityAuditSeverity::High);

    let exit_code = if has_critical {
        2
    } else if has_high {
        1
    } else {
        0
    };

    let output = match format {
        AuditOutputFormat::Json => format_json(&report),
        AuditOutputFormat::HumanReadable => format_human_readable(&report),
    };

    (output, exit_code)
}

// ---------------------------------------------------------------------------
// Formatters
// ---------------------------------------------------------------------------

/// Format the audit report as pretty-printed JSON.
pub fn format_json(report: &SecurityAuditReport) -> String {
    let enriched = json!({
        "timestamp": report.timestamp.to_rfc3339(),
        "summary": {
            "total_checks": report.total_checks,
            "passed": report.passed_count,
            "findings": report.findings.len(),
        },
        "findings": report.findings,
    });

    serde_json::to_string_pretty(&enriched)
        .unwrap_or_else(|e| format!("{{\"error\": \"Failed to serialize report: {}\"}}", e))
}

/// Format the audit report as human-readable text with ANSI colours.
pub fn format_human_readable(report: &SecurityAuditReport) -> String {
    let mut out = String::with_capacity(2048);

    // Header
    out.push_str(&format!(
        "\n{}{}NexiBot Security Audit{}\n",
        ANSI_BOLD, ANSI_CYAN, ANSI_RESET
    ));
    out.push_str(&format!(
        "{}Timestamp: {}{}\n\n",
        ANSI_DIM,
        report.timestamp.format("%Y-%m-%d %H:%M:%S UTC"),
        ANSI_RESET
    ));

    // Summary
    let finding_count = report.findings.len();
    let summary_color = if finding_count == 0 {
        ANSI_GREEN
    } else {
        ANSI_YELLOW
    };
    out.push_str(&format!(
        "{}Summary:{} {}{}/{} checks passed{} ({} finding{})\n\n",
        ANSI_BOLD,
        ANSI_RESET,
        summary_color,
        report.passed_count,
        report.total_checks,
        ANSI_RESET,
        finding_count,
        if finding_count == 1 { "" } else { "s" },
    ));

    if report.findings.is_empty() {
        out.push_str(&format!(
            "  {}All checks passed. No security issues found.{}\n\n",
            ANSI_GREEN, ANSI_RESET
        ));
        return out;
    }

    // Group findings by severity (Critical first, then High, Medium, Low, Info)
    let severity_order = [
        SecurityAuditSeverity::Critical,
        SecurityAuditSeverity::High,
        SecurityAuditSeverity::Medium,
        SecurityAuditSeverity::Low,
        SecurityAuditSeverity::Info,
    ];

    for severity in &severity_order {
        let group: Vec<_> = report
            .findings
            .iter()
            .filter(|f| f.severity == *severity)
            .collect();

        if group.is_empty() {
            continue;
        }

        let color = severity_color(severity);
        out.push_str(&format!(
            "{}{}{} {} ({}):{}\n",
            ANSI_BOLD,
            color,
            severity_icon(severity),
            severity,
            group.len(),
            ANSI_RESET
        ));

        for finding in &group {
            out.push_str(&format!(
                "  {}[{}]{} {}\n",
                color, finding.id, ANSI_RESET, finding.title
            ));
            out.push_str(&format!(
                "    {}{}{}\n",
                ANSI_DIM, finding.description, ANSI_RESET
            ));

            if let Some(hint) = &finding.fix_hint {
                out.push_str(&format!("    {}Fix: {}{}\n", ANSI_GREEN, hint, ANSI_RESET));
            }

            if finding.auto_fixable {
                out.push_str(&format!("    {}(auto-fixable){}\n", ANSI_CYAN, ANSI_RESET));
            }

            out.push('\n');
        }
    }

    // Auto-fixable summary
    let auto_fixable_count = report.findings.iter().filter(|f| f.auto_fixable).count();
    if auto_fixable_count > 0 {
        out.push_str(&format!(
            "{}{} finding{} can be auto-fixed.{} Run with --auto-fix to apply.\n\n",
            ANSI_CYAN,
            auto_fixable_count,
            if auto_fixable_count == 1 { "" } else { "s" },
            ANSI_RESET
        ));
    }

    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::audit::{
        SecurityAuditFinding, SecurityAuditReport, SecurityAuditSeverity,
    };
    use chrono::Utc;

    fn make_report_no_findings() -> SecurityAuditReport {
        SecurityAuditReport {
            findings: vec![],
            passed_count: 10,
            total_checks: 10,
            timestamp: Utc::now(),
        }
    }

    fn make_report_with_findings() -> SecurityAuditReport {
        SecurityAuditReport {
            findings: vec![
                SecurityAuditFinding {
                    id: "cfg-api-key".into(),
                    severity: SecurityAuditSeverity::Critical,
                    title: "No API key configured".into(),
                    description: "No LLM provider API key is set.".into(),
                    fix_hint: Some("Add an API key.".into()),
                    auto_fixable: false,
                },
                SecurityAuditFinding {
                    id: "cfg-defense-disabled".into(),
                    severity: SecurityAuditSeverity::High,
                    title: "Defense pipeline disabled".into(),
                    description: "The defense pipeline is disabled.".into(),
                    fix_hint: Some("Set defense.enabled = true.".into()),
                    auto_fixable: true,
                },
                SecurityAuditFinding {
                    id: "cfg-ssrf-blocked-domains".into(),
                    severity: SecurityAuditSeverity::Medium,
                    title: "Fetch tool has no blocked domains".into(),
                    description: "No SSRF protection.".into(),
                    fix_hint: None,
                    auto_fixable: true,
                },
                SecurityAuditFinding {
                    id: "rt-session-encryption".into(),
                    severity: SecurityAuditSeverity::Info,
                    title: "Session data stored unencrypted".into(),
                    description: "Sessions stored as plaintext.".into(),
                    fix_hint: Some("Use full-disk encryption.".into()),
                    auto_fixable: false,
                },
            ],
            passed_count: 6,
            total_checks: 10,
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn test_format_json_no_findings() {
        let report = make_report_no_findings();
        let json = format_json(&report);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["summary"]["findings"], 0);
        assert_eq!(parsed["summary"]["passed"], 10);
    }

    #[test]
    fn test_format_json_with_findings() {
        let report = make_report_with_findings();
        let json = format_json(&report);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["summary"]["findings"], 4);
        assert!(parsed["findings"].is_array());
    }

    #[test]
    fn test_format_human_readable_no_findings() {
        let report = make_report_no_findings();
        let text = format_human_readable(&report);
        assert!(text.contains("All checks passed"));
        assert!(text.contains("10/10"));
    }

    #[test]
    fn test_format_human_readable_with_findings() {
        let report = make_report_with_findings();
        let text = format_human_readable(&report);
        // Should contain severity headers
        assert!(text.contains("Critical"));
        assert!(text.contains("High"));
        assert!(text.contains("Medium"));
        assert!(text.contains("Info"));
        // Should contain finding IDs
        assert!(text.contains("cfg-api-key"));
        assert!(text.contains("cfg-defense-disabled"));
        // Should mention auto-fixable
        assert!(text.contains("auto-fix"));
    }

    #[test]
    fn test_format_human_readable_fix_hints() {
        let report = make_report_with_findings();
        let text = format_human_readable(&report);
        assert!(text.contains("Fix: Add an API key."));
        assert!(text.contains("Fix: Set defense.enabled = true."));
    }

    #[test]
    fn test_exit_code_no_issues() {
        let report = make_report_no_findings();
        let has_critical = report
            .findings
            .iter()
            .any(|f| f.severity == SecurityAuditSeverity::Critical);
        let has_high = report
            .findings
            .iter()
            .any(|f| f.severity == SecurityAuditSeverity::High);
        let code = if has_critical {
            2
        } else if has_high {
            1
        } else {
            0
        };
        assert_eq!(code, 0);
    }

    #[test]
    fn test_exit_code_high() {
        let report = SecurityAuditReport {
            findings: vec![SecurityAuditFinding {
                id: "test".into(),
                severity: SecurityAuditSeverity::High,
                title: "Test".into(),
                description: "Test".into(),
                fix_hint: None,
                auto_fixable: false,
            }],
            passed_count: 9,
            total_checks: 10,
            timestamp: Utc::now(),
        };
        let has_critical = report
            .findings
            .iter()
            .any(|f| f.severity == SecurityAuditSeverity::Critical);
        let has_high = report
            .findings
            .iter()
            .any(|f| f.severity == SecurityAuditSeverity::High);
        let code = if has_critical {
            2
        } else if has_high {
            1
        } else {
            0
        };
        assert_eq!(code, 1);
    }

    #[test]
    fn test_exit_code_critical() {
        let report = SecurityAuditReport {
            findings: vec![SecurityAuditFinding {
                id: "test".into(),
                severity: SecurityAuditSeverity::Critical,
                title: "Test".into(),
                description: "Test".into(),
                fix_hint: None,
                auto_fixable: false,
            }],
            passed_count: 9,
            total_checks: 10,
            timestamp: Utc::now(),
        };
        let has_critical = report
            .findings
            .iter()
            .any(|f| f.severity == SecurityAuditSeverity::Critical);
        let has_high = report
            .findings
            .iter()
            .any(|f| f.severity == SecurityAuditSeverity::High);
        let code = if has_critical {
            2
        } else if has_high {
            1
        } else {
            0
        };
        assert_eq!(code, 2);
    }

    #[test]
    fn test_severity_color_mapping() {
        assert_eq!(severity_color(&SecurityAuditSeverity::Critical), ANSI_RED);
        assert_eq!(severity_color(&SecurityAuditSeverity::High), ANSI_YELLOW);
        assert_eq!(severity_color(&SecurityAuditSeverity::Medium), ANSI_BLUE);
        assert_eq!(severity_color(&SecurityAuditSeverity::Low), ANSI_CYAN);
        assert_eq!(severity_color(&SecurityAuditSeverity::Info), ANSI_DIM);
    }

    #[test]
    fn test_severity_icon_mapping() {
        assert_eq!(severity_icon(&SecurityAuditSeverity::Critical), "[!!]");
        assert_eq!(severity_icon(&SecurityAuditSeverity::High), "[! ]");
        assert_eq!(severity_icon(&SecurityAuditSeverity::Medium), "[* ]");
        assert_eq!(severity_icon(&SecurityAuditSeverity::Low), "[- ]");
        assert_eq!(severity_icon(&SecurityAuditSeverity::Info), "[i ]");
    }

    #[test]
    fn test_auto_fixable_summary_count() {
        let report = make_report_with_findings();
        let text = format_human_readable(&report);
        // 2 auto-fixable findings in our test data
        assert!(text.contains("2 findings can be auto-fixed."));
    }
}
