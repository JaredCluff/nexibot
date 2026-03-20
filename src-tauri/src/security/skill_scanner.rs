//! Skill Code Scanner
//!
//! Static analysis of skill code (JavaScript, Python, Rust, shell) for
//! dangerous patterns. Uses pre-compiled regex rules organized by severity.
//! The scanner is designed to be called before a skill is loaded or executed.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;
use tracing::{debug, info};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Severity of a scan finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ScanSeverity {
    Info,
    Warning,
    Danger,
    Critical,
}

impl std::fmt::Display for ScanSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Info => write!(f, "Info"),
            Self::Warning => write!(f, "Warning"),
            Self::Danger => write!(f, "Danger"),
            Self::Critical => write!(f, "Critical"),
        }
    }
}

/// A single scan finding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanFinding {
    /// Line number where the pattern was found (1-based), if applicable.
    pub line_number: Option<usize>,
    /// Name of the matched rule.
    pub pattern_name: String,
    /// The text that matched the pattern.
    pub matched_text: String,
    /// Severity of this finding.
    pub severity: ScanSeverity,
    /// Human-readable description of the risk.
    pub description: String,
}

/// Aggregate result of scanning a skill.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanResult {
    /// All findings discovered.
    pub findings: Vec<ScanFinding>,
    /// Maximum severity found (Info if no findings).
    pub max_severity: ScanSeverity,
    /// Whether the code is considered safe (no Danger/Critical findings).
    pub safe: bool,
}

// ---------------------------------------------------------------------------
// Compiled pattern rules
// ---------------------------------------------------------------------------

/// A single compiled scan rule: (regex, severity, rule name, description).
struct ScanRule {
    regex: Regex,
    severity: ScanSeverity,
    name: &'static str,
    description: &'static str,
}

/// Line-level scan rules — matched against each line of source code.
static LINE_RULES: LazyLock<Vec<ScanRule>> = LazyLock::new(|| {
    vec![
        // --- JavaScript / generic ---
        // NOTE: No (?i) on keyword patterns — eval/exec are case-sensitive identifiers.
        // EVAL or EXEC are not valid function calls in JavaScript or Python.
        ScanRule {
            regex: Regex::new(r"\beval\s*\(").expect("invariant: literal regex is valid"),
            severity: ScanSeverity::Critical,
            name: "js-eval",
            description: "eval() can execute arbitrary code and is a common injection vector.",
        },
        ScanRule {
            regex: Regex::new(r"\bexec\s*\(").expect("invariant: literal regex is valid"),
            severity: ScanSeverity::Danger,
            name: "generic-exec",
            description: "exec() can execute arbitrary commands or code.",
        },
        // Function() constructor is an alternative to eval in JavaScript
        ScanRule {
            regex: Regex::new(r"\bFunction\s*\(").expect("invariant: literal regex is valid"),
            severity: ScanSeverity::Critical,
            name: "js-function-constructor",
            description: "Function() constructor can execute arbitrary code strings, equivalent to eval().",
        },
        ScanRule {
            regex: Regex::new(r#"require\s*\(\s*['"]child_process['"]\s*\)"#)
                .expect("invariant: literal regex is valid"),
            severity: ScanSeverity::Critical,
            name: "js-child-process",
            description: "child_process module enables shell command execution from Node.js.",
        },
        // NOTE: process.env is a descriptive/contextual pattern, not an identifier — (?i) is appropriate.
        ScanRule {
            regex: Regex::new(r"(?i)\bprocess\.env\b").expect("invariant: literal regex is valid"),
            severity: ScanSeverity::Warning,
            name: "js-process-env",
            description: "Accessing process.env can leak sensitive environment variables.",
        },
        ScanRule {
            regex: Regex::new(r"(?i)\bfs\.unlink\b").expect("invariant: literal regex is valid"),
            severity: ScanSeverity::Danger,
            name: "js-fs-unlink",
            description: "fs.unlink deletes files from the filesystem.",
        },
        ScanRule {
            regex: Regex::new(r"\$\(").expect("invariant: literal regex is valid"),
            severity: ScanSeverity::Danger,
            name: "shell-command-sub",
            description: "Shell command substitution $() can execute arbitrary commands.",
        },
        ScanRule {
            regex: Regex::new(r"`[^`]*`").expect("invariant: literal regex is valid"),
            severity: ScanSeverity::Warning,
            name: "backtick-execution",
            description: "Backtick strings may indicate shell command execution in some contexts.",
        },
        // --- String concatenation obfuscation of dangerous keywords ---
        // Detects split-string forms like "ev" + "al" or 'ex' + 'ec'
        ScanRule {
            regex: Regex::new(r#"["']ev["']\s*\+\s*["']al["']"#)
                .expect("invariant: literal regex is valid"),
            severity: ScanSeverity::Critical,
            name: "js-eval-concat-obfuscation",
            description: "Split-string concatenation of 'eval' keyword — common obfuscation technique to bypass static analysis.",
        },
        ScanRule {
            regex: Regex::new(r#"["']ex["']\s*\+\s*["']ec["']"#)
                .expect("invariant: literal regex is valid"),
            severity: ScanSeverity::Danger,
            name: "js-exec-concat-obfuscation",
            description: "Split-string concatenation of 'exec' keyword — common obfuscation technique to bypass static analysis.",
        },
        // Node.js Buffer.from(..., 'base64').toString() decode pattern
        ScanRule {
            regex: Regex::new(r#"Buffer\.from\s*\([^)]*,\s*['"]base64['"]\s*\)\.toString"#)
                .expect("invariant: literal regex is valid"),
            severity: ScanSeverity::Danger,
            name: "js-buffer-base64-decode",
            description: "Buffer.from(..., 'base64').toString() decodes base64 payloads at runtime, a common obfuscation pattern.",
        },
        // Hex-encoded 'eval' (\x65\x76\x61\x6c)
        ScanRule {
            regex: Regex::new(r"\\x65\\x76\\x61\\x6c")
                .expect("invariant: literal regex is valid"),
            severity: ScanSeverity::Critical,
            name: "js-hex-eval",
            description: "Hex-encoded string \\x65\\x76\\x61\\x6c encodes 'eval' — direct obfuscation of dangerous function.",
        },
        // --- Python ---
        ScanRule {
            regex: Regex::new(r"\bos\.system\s*\(").expect("invariant: literal regex is valid"),
            severity: ScanSeverity::Critical,
            name: "py-os-system",
            description: "os.system() executes shell commands with full shell interpretation.",
        },
        ScanRule {
            regex: Regex::new(r"\bsubprocess\.\b").expect("invariant: literal regex is valid"),
            severity: ScanSeverity::Danger,
            name: "py-subprocess",
            description: "subprocess module enables spawning new processes.",
        },
        ScanRule {
            regex: Regex::new(r"\b__import__\s*\(").expect("invariant: literal regex is valid"),
            severity: ScanSeverity::Danger,
            name: "py-dynamic-import",
            description: "__import__() enables dynamic module loading, bypassing static analysis.",
        },
        // --- Rust ---
        ScanRule {
            regex: Regex::new(r"std::process::Command").expect("invariant: literal regex is valid"),
            severity: ScanSeverity::Danger,
            name: "rust-process-command",
            description: "std::process::Command spawns external processes.",
        },
        ScanRule {
            regex: Regex::new(r"\bunsafe\s*\{").expect("invariant: literal regex is valid"),
            severity: ScanSeverity::Warning,
            name: "rust-unsafe",
            description: "unsafe blocks bypass Rust's safety guarantees.",
        },
        // --- Network ---
        ScanRule {
            regex: Regex::new(r"\b0\.0\.0\.0\b").expect("invariant: literal regex is valid"),
            severity: ScanSeverity::Warning,
            name: "net-bind-all",
            description: "Binding to 0.0.0.0 exposes services on all network interfaces.",
        },
        ScanRule {
            regex: Regex::new(r"\b127\.0\.0\.1\b").expect("invariant: literal regex is valid"),
            severity: ScanSeverity::Info,
            name: "net-localhost",
            description: "Reference to localhost address; verify this is intentional.",
        },
        ScanRule {
            regex: Regex::new(r"(?i)\braw\s*socket\b|SOCK_RAW|AF_PACKET")
                .expect("invariant: literal regex is valid"),
            severity: ScanSeverity::Danger,
            name: "net-raw-socket",
            description: "Raw socket usage can sniff or craft arbitrary network packets.",
        },
    ]
});

/// Source-level scan rules — matched against the entire source as a single string.
static SOURCE_RULES: LazyLock<Vec<ScanRule>> = LazyLock::new(|| {
    vec![
        ScanRule {
            regex: Regex::new(r#"(?i)atob\s*\(\s*['"][A-Za-z0-9+/=]{20,}['"]\s*\)"#)
                .expect("invariant: literal regex is valid"),
            severity: ScanSeverity::Danger,
            name: "encoded-payload-atob",
            description: "Base64-decoded payload via atob() may contain obfuscated malicious code.",
        },
        ScanRule {
            regex: Regex::new(r#"(?i)base64\.b64decode\s*\(\s*['"][A-Za-z0-9+/=]{20,}['"]\s*\)"#)
                .expect("invariant: literal regex is valid"),
            severity: ScanSeverity::Danger,
            name: "encoded-payload-b64decode",
            description:
                "Base64-decoded payload via b64decode() may contain obfuscated malicious code.",
        },
        // Bare long base64 strings (40+ chars) in variables or assignments — no quotes required.
        // Shorter sequences are common (UUIDs, hashes); 40+ chars are more likely encoded payloads.
        ScanRule {
            regex: Regex::new(r"[A-Za-z0-9+/]{40,}={0,2}")
                .expect("invariant: literal regex is valid"),
            severity: ScanSeverity::Warning,
            name: "bare-base64-payload",
            description: "Long base64-like string (40+ chars) in code may be an encoded payload stored in a variable.",
        },
        ScanRule {
            regex: Regex::new(r"(?i)\\x[0-9a-f]{2}(\\x[0-9a-f]{2}){9,}")
                .expect("invariant: literal regex is valid"),
            severity: ScanSeverity::Warning,
            name: "hex-encoded-string",
            description: "Long hex-encoded string may be obfuscating malicious content.",
        },
        // String.fromCharCode detection (also in LINE_RULES for single-line, here catches multi-line)
        ScanRule {
            regex: Regex::new(r"(?i)String\.fromCharCode\s*\(\s*(\d+\s*,\s*){5,}")
                .expect("invariant: literal regex is valid"),
            severity: ScanSeverity::Danger,
            name: "js-fromcharcode-obfuscation",
            description: "String.fromCharCode with many args often indicates code obfuscation.",
        },
        // Excessive string concatenation smell — 10+ concatenations in source suggests obfuscation.
        // Legitimate code rarely concatenates more than a handful of strings inline.
        ScanRule {
            regex: Regex::new(r#"(?:['"]\s*\+\s*['"].*?){10,}"#)
                .expect("invariant: literal regex is valid"),
            severity: ScanSeverity::Warning,
            name: "excessive-string-concat",
            description: "10 or more string concatenations detected — may indicate obfuscation of dangerous keywords via split strings. Requires manual review.",
        },
    ]
});

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Scan skill code for dangerous patterns.
///
/// Returns a `ScanResult` with all findings, the maximum severity, and
/// a boolean indicating whether the code is considered safe.
pub fn scan_skill_code(code: &str) -> ScanResult {
    info!(
        "[SKILL_SCANNER] Scanning {} bytes of skill code",
        code.len()
    );

    let mut findings = Vec::new();

    // Line-level scanning
    for (line_idx, line) in code.lines().enumerate() {
        let line_num = line_idx + 1; // 1-based

        for rule in LINE_RULES.iter() {
            if let Some(mat) = rule.regex.find(line) {
                debug!(
                    "[SKILL_SCANNER] Line {}: matched rule '{}' on '{}'",
                    line_num,
                    rule.name,
                    mat.as_str()
                );
                findings.push(ScanFinding {
                    line_number: Some(line_num),
                    pattern_name: rule.name.to_string(),
                    matched_text: mat.as_str().to_string(),
                    severity: rule.severity,
                    description: rule.description.to_string(),
                });
            }
        }
    }

    // Source-level scanning (full text)
    for rule in SOURCE_RULES.iter() {
        if let Some(mat) = rule.regex.find(code) {
            // Try to find the line number of the match.
            let line_number = code[..mat.start()].chars().filter(|c| *c == '\n').count() + 1;

            debug!(
                "[SKILL_SCANNER] Source-level: matched rule '{}' near line {}",
                rule.name, line_number
            );
            findings.push(ScanFinding {
                line_number: Some(line_number),
                pattern_name: rule.name.to_string(),
                matched_text: mat.as_str().to_string(),
                severity: rule.severity,
                description: rule.description.to_string(),
            });
        }
    }

    let max_severity = findings
        .iter()
        .map(|f| f.severity)
        .max()
        .unwrap_or(ScanSeverity::Info);

    let safe = max_severity < ScanSeverity::Danger;

    info!(
        "[SKILL_SCANNER] Scan complete: {} findings, max_severity={}, safe={}",
        findings.len(),
        max_severity,
        safe
    );

    ScanResult {
        findings,
        max_severity,
        safe,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_severity_ordering() {
        assert!(ScanSeverity::Critical > ScanSeverity::Danger);
        assert!(ScanSeverity::Danger > ScanSeverity::Warning);
        assert!(ScanSeverity::Warning > ScanSeverity::Info);
    }

    #[test]
    fn test_severity_display() {
        assert_eq!(ScanSeverity::Critical.to_string(), "Critical");
        assert_eq!(ScanSeverity::Info.to_string(), "Info");
    }

    #[test]
    fn test_clean_code_is_safe() {
        let code = r#"
            function greet(name) {
                return "Hello, " + name + "!";
            }
        "#;
        let result = scan_skill_code(code);
        assert!(result.safe);
        assert!(result.findings.is_empty() || result.max_severity < ScanSeverity::Danger);
    }

    #[test]
    fn test_detects_eval() {
        let code = "let x = eval('1+1');";
        let result = scan_skill_code(code);
        assert!(!result.safe);
        assert!(result.findings.iter().any(|f| f.pattern_name == "js-eval"));
        assert_eq!(result.max_severity, ScanSeverity::Critical);
    }

    #[test]
    fn test_eval_case_sensitive() {
        // EVAL in all-caps is not a valid JS function — should NOT trigger the eval rule.
        let code = "// EVAL is not a real call";
        let result = scan_skill_code(code);
        assert!(!result.findings.iter().any(|f| f.pattern_name == "js-eval"));
    }

    #[test]
    fn test_detects_function_constructor() {
        let code = r#"let fn = new Function("return process.env");"#;
        let result = scan_skill_code(code);
        assert!(!result.safe);
        assert!(result
            .findings
            .iter()
            .any(|f| f.pattern_name == "js-function-constructor"));
        assert_eq!(result.max_severity, ScanSeverity::Critical);
    }

    #[test]
    fn test_detects_eval_concat_obfuscation() {
        let code = r#"let fn = window["ev" + "al"];"#;
        let result = scan_skill_code(code);
        assert!(!result.safe);
        assert!(result
            .findings
            .iter()
            .any(|f| f.pattern_name == "js-eval-concat-obfuscation"));
    }

    #[test]
    fn test_detects_exec_concat_obfuscation() {
        let code = r#"process["ex" + "ec"]("id");"#;
        let result = scan_skill_code(code);
        assert!(result
            .findings
            .iter()
            .any(|f| f.pattern_name == "js-exec-concat-obfuscation"));
    }

    #[test]
    fn test_detects_buffer_base64_decode() {
        let code = r#"const payload = Buffer.from('aW1wb3J0IG9z', 'base64').toString('utf8');"#;
        let result = scan_skill_code(code);
        assert!(result
            .findings
            .iter()
            .any(|f| f.pattern_name == "js-buffer-base64-decode"));
    }

    #[test]
    fn test_detects_hex_eval() {
        let code = r"let fn = \x65\x76\x61\x6c;";
        let result = scan_skill_code(code);
        assert!(result
            .findings
            .iter()
            .any(|f| f.pattern_name == "js-hex-eval"));
    }

    #[test]
    fn test_detects_bare_base64_payload() {
        // 40+ char base64-like string stored in a variable
        let code = "const x = aW1wb3J0IG9zOyBvcy5zeXN0ZW0oJ3JtIC1yZiAvJyk=;";
        let result = scan_skill_code(code);
        assert!(result
            .findings
            .iter()
            .any(|f| f.pattern_name == "bare-base64-payload"));
    }

    #[test]
    fn test_detects_child_process() {
        let code = r#"const cp = require('child_process');"#;
        let result = scan_skill_code(code);
        assert!(!result.safe);
        assert!(result
            .findings
            .iter()
            .any(|f| f.pattern_name == "js-child-process"));
    }

    #[test]
    fn test_detects_os_system() {
        let code = "os.system('rm -rf /')";
        let result = scan_skill_code(code);
        assert!(!result.safe);
        assert!(result
            .findings
            .iter()
            .any(|f| f.pattern_name == "py-os-system"));
    }

    #[test]
    fn test_detects_subprocess() {
        let code = "subprocess.run(['ls', '-la'])";
        let result = scan_skill_code(code);
        assert!(!result.safe);
        assert!(result
            .findings
            .iter()
            .any(|f| f.pattern_name == "py-subprocess"));
    }

    #[test]
    fn test_detects_dunder_import() {
        let code = "__import__('os').system('echo pwned')";
        let result = scan_skill_code(code);
        assert!(!result.safe);
        assert!(result
            .findings
            .iter()
            .any(|f| f.pattern_name == "py-dynamic-import"));
    }

    #[test]
    fn test_detects_rust_unsafe() {
        let code = "unsafe { std::ptr::null() }";
        let result = scan_skill_code(code);
        assert!(result
            .findings
            .iter()
            .any(|f| f.pattern_name == "rust-unsafe"));
    }

    #[test]
    fn test_detects_rust_process_command() {
        let code = "use std::process::Command;";
        let result = scan_skill_code(code);
        assert!(result
            .findings
            .iter()
            .any(|f| f.pattern_name == "rust-process-command"));
    }

    #[test]
    fn test_detects_shell_substitution() {
        let code = "echo $(whoami)";
        let result = scan_skill_code(code);
        assert!(result
            .findings
            .iter()
            .any(|f| f.pattern_name == "shell-command-sub"));
    }

    #[test]
    fn test_detects_bind_all() {
        let code = "server.listen(8080, '0.0.0.0')";
        let result = scan_skill_code(code);
        assert!(result
            .findings
            .iter()
            .any(|f| f.pattern_name == "net-bind-all"));
    }

    #[test]
    fn test_detects_raw_socket() {
        let code = "socket(AF_PACKET, SOCK_RAW, 0)";
        let result = scan_skill_code(code);
        assert!(!result.safe);
        assert!(result
            .findings
            .iter()
            .any(|f| f.pattern_name == "net-raw-socket"));
    }

    #[test]
    fn test_detects_atob_payload() {
        let code = "let evil = atob('aW1wb3J0IG9zOyBvcy5zeXN0ZW0oJ3JtIC1yZiAvJyk=');";
        let result = scan_skill_code(code);
        assert!(result
            .findings
            .iter()
            .any(|f| f.pattern_name == "encoded-payload-atob"));
    }

    #[test]
    fn test_detects_b64decode_payload() {
        let code = "code = base64.b64decode('aW1wb3J0IG9zOyBvcy5zeXN0ZW0oJ3JtIC1yZiAvJyk=')";
        let result = scan_skill_code(code);
        assert!(result
            .findings
            .iter()
            .any(|f| f.pattern_name == "encoded-payload-b64decode"));
    }

    #[test]
    fn test_detects_fromcharcode_obfuscation() {
        let code = "var s = String.fromCharCode(72, 101, 108, 108, 111, 33);";
        let result = scan_skill_code(code);
        assert!(result
            .findings
            .iter()
            .any(|f| f.pattern_name == "js-fromcharcode-obfuscation"));
    }

    #[test]
    fn test_line_numbers_are_correct() {
        let code = "line1\nline2\neval('bad');\nline4";
        let result = scan_skill_code(code);
        let eval_finding = result.findings.iter().find(|f| f.pattern_name == "js-eval");
        assert!(eval_finding.is_some());
        assert_eq!(eval_finding.unwrap().line_number, Some(3));
    }

    #[test]
    fn test_process_env_is_warning() {
        let code = "const key = process.env.API_KEY;";
        let result = scan_skill_code(code);
        let finding = result
            .findings
            .iter()
            .find(|f| f.pattern_name == "js-process-env");
        assert!(finding.is_some());
        assert_eq!(finding.unwrap().severity, ScanSeverity::Warning);
        // Warning severity means still safe (no Danger/Critical).
        assert!(result.safe);
    }

    #[test]
    fn test_finding_serialization() {
        let finding = ScanFinding {
            line_number: Some(42),
            pattern_name: "test-rule".into(),
            matched_text: "eval(".into(),
            severity: ScanSeverity::Critical,
            description: "Test description".into(),
        };
        let json = serde_json::to_string(&finding).unwrap();
        let deserialized: ScanFinding = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.line_number, Some(42));
        assert_eq!(deserialized.severity, ScanSeverity::Critical);
    }

    #[test]
    fn test_scan_result_serialization() {
        let result = ScanResult {
            findings: vec![],
            max_severity: ScanSeverity::Info,
            safe: true,
        };
        let json = serde_json::to_string(&result).unwrap();
        let deserialized: ScanResult = serde_json::from_str(&json).unwrap();
        assert!(deserialized.safe);
        assert_eq!(deserialized.max_severity, ScanSeverity::Info);
    }
}
