//! Security audit CLI subcommand.
//!
//! Exposes the 17-check security audit system via the CLI.
//! Supports standard audit, deep audit, auto-fix, and JSON output.

use clap::{Args, Subcommand};
use colored::Colorize;

use crate::client::NexiBotClient;
use crate::error::CliError;
use crate::output::{self, format_output};

#[derive(Args)]
pub struct SecurityArgs {
    #[command(subcommand)]
    pub command: SecurityCommands,
}

#[derive(Subcommand)]
pub enum SecurityCommands {
    /// Run a security audit
    Audit {
        /// Include runtime checks (takes longer)
        #[arg(long)]
        deep: bool,

        /// Auto-fix all fixable findings
        #[arg(long)]
        fix: bool,

        /// Output format (json or table)
        #[arg(long, default_value = "table")]
        format: String,
    },
}

pub async fn handle(args: SecurityArgs, client: &NexiBotClient) -> Result<(), CliError> {
    match args.command {
        SecurityCommands::Audit { deep, fix, format } => {
            handle_audit(client, deep, fix, &format).await
        }
    }
}

async fn handle_audit(
    client: &NexiBotClient,
    deep: bool,
    fix: bool,
    format: &str,
) -> Result<(), CliError> {
    if !client.health_check().await? {
        return Err(CliError::ServerUnreachable);
    }

    output::info("Running security audit...");

    let response = client.run_security_audit(deep, fix).await?;

    match format {
        "json" => {
            println!("{}", format_output(&response, "json"));
        }
        _ => {
            format_audit_table(&response);
        }
    }

    Ok(())
}

fn format_audit_table(report: &serde_json::Value) {
    println!();
    println!("{}", "NexiBot Security Audit".cyan().bold());

    if let Some(timestamp) = report.get("timestamp").and_then(|t| t.as_str()) {
        println!("{}", format!("Timestamp: {}", timestamp).dimmed());
    }
    println!();

    // Summary
    if let Some(summary) = report.get("summary") {
        let total = summary
            .get("total_checks")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let passed = summary.get("passed").and_then(|v| v.as_u64()).unwrap_or(0);
        let findings = summary
            .get("findings")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let summary_text = format!("{}/{} checks passed ({} findings)", passed, total, findings);
        if findings == 0 {
            println!("Summary: {}", summary_text.green());
        } else {
            println!("Summary: {}", summary_text.yellow());
        }
        println!();
    }

    // Findings
    if let Some(findings) = report.get("findings").and_then(|f| f.as_array()) {
        if findings.is_empty() {
            println!(
                "  {}",
                "All checks passed. No security issues found.".green()
            );
            println!();
            return;
        }

        for finding in findings {
            let severity = finding
                .get("severity")
                .and_then(|s| s.as_str())
                .unwrap_or("Unknown");
            let id = finding.get("id").and_then(|s| s.as_str()).unwrap_or("");
            let title = finding.get("title").and_then(|s| s.as_str()).unwrap_or("");
            let description = finding
                .get("description")
                .and_then(|s| s.as_str())
                .unwrap_or("");
            let auto_fixable = finding
                .get("auto_fixable")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let severity_display = match severity {
                "Critical" => format!("[!!] {}", severity).red().bold().to_string(),
                "High" => format!("[! ] {}", severity).yellow().bold().to_string(),
                "Medium" => format!("[* ] {}", severity).blue().to_string(),
                "Low" => format!("[- ] {}", severity).cyan().to_string(),
                "Info" => format!("[i ] {}", severity).dimmed().to_string(),
                _ => severity.to_string(),
            };

            println!("  {} [{}] {}", severity_display, id, title);
            println!("    {}", description.dimmed());

            if let Some(hint) = finding.get("fix_hint").and_then(|s| s.as_str()) {
                println!("    {}", format!("Fix: {}", hint).green());
            }

            if auto_fixable {
                println!("    {}", "(auto-fixable)".cyan());
            }

            println!();
        }

        // Auto-fixable summary
        let auto_fixable_count = findings
            .iter()
            .filter(|f| {
                f.get("auto_fixable")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            })
            .count();
        if auto_fixable_count > 0 {
            println!(
                "{}",
                format!(
                    "{} finding{} can be auto-fixed. Run with --fix to apply.",
                    auto_fixable_count,
                    if auto_fixable_count == 1 { "" } else { "s" }
                )
                .cyan()
            );
            println!();
        }
    }
}
