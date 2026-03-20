//! Binary path validation and trusted directory allowlist.
//!
//! Prevents PATH hijacking by resolving binaries to absolute paths
//! and verifying they reside in trusted directories.

use std::path::{Path, PathBuf};
use tracing::debug;

/// Hardcoded trusted system directories for binary resolution.
/// Only well-known system directories are trusted to prevent PATH hijacking.
#[cfg(not(windows))]
const TRUSTED_SYSTEM_DIRS: &[&str] = &["/bin", "/usr/bin", "/usr/local/bin", "/sbin", "/usr/sbin"];

#[cfg(windows)]
const TRUSTED_SYSTEM_DIRS: &[&str] = &[
    r"C:\Windows\System32",
    r"C:\Windows",
    r"C:\Windows\System32\WindowsPowerShell\v1.0",
    r"C:\Program Files",
    r"C:\Program Files (x86)",
];

/// Blocked output/file-reading flags that could be used for oracle attacks
/// or unauthorized file writes. Each entry is (binary_name, blocked_flag).
#[allow(dead_code)]
const BLOCKED_OUTPUT_FLAGS: &[(&str, &str)] = &[
    ("sort", "-o"),
    ("sort", "--output"),
    ("jq", "-f"),
    ("jq", "--from-file"),
    ("grep", "-f"),
    ("grep", "--file"),
    ("awk", "-f"),
    ("awk", "--file"),
    ("sed", "-f"),
    ("sed", "--file"),
    ("tee", "-a"),
    ("tee", "--append"),
    ("dd", "of="),
    ("xargs", "-o"),
];

/// Build the set of trusted directories from the hardcoded system list only.
/// Does NOT inherit directories from the PATH environment variable to prevent
/// PATH hijacking attacks.
pub fn build_trusted_dirs() -> Vec<PathBuf> {
    TRUSTED_SYSTEM_DIRS
        .iter()
        .map(|d| PathBuf::from(d))
        .collect()
}

/// Check whether a set of command-line arguments contains blocked output/file flags
/// for the given binary name.
///
/// Returns `Err` with a description if a blocked flag is found.
#[allow(dead_code)]
pub fn check_blocked_flags(binary_name: &str, args: &[&str]) -> Result<(), String> {
    let bin_basename = Path::new(binary_name)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(binary_name);

    for (blocked_bin, blocked_flag) in BLOCKED_OUTPUT_FLAGS {
        if bin_basename != *blocked_bin {
            continue;
        }
        for arg in args {
            // Exact match or prefix match (for flags like "of=")
            if *arg == *blocked_flag || arg.starts_with(blocked_flag) {
                return Err(format!(
                    "Blocked flag '{}' for binary '{}': potential file write or oracle attack",
                    blocked_flag, bin_basename
                ));
            }
        }
    }
    Ok(())
}

/// Resolve a binary name to an absolute path by searching trusted system directories only.
///
/// Returns the first matching executable found in a trusted directory.
pub fn resolve_binary(name: &str) -> Option<PathBuf> {
    // If already absolute, return as-is
    let path = Path::new(name);
    if path.is_absolute() {
        return if path.exists() {
            Some(path.to_path_buf())
        } else {
            None
        };
    }

    // Search only trusted system directories (not PATH)
    let trusted_dirs = build_trusted_dirs();
    for dir in &trusted_dirs {
        let candidate = dir.join(name);
        if candidate.is_file() {
            // Canonicalize to resolve symlinks
            return std::fs::canonicalize(&candidate).ok();
        }
    }

    None
}

/// Resolve a binary and verify it resides in a trusted directory.
///
/// Returns the absolute path if the binary is found and trusted,
/// or an error describing why validation failed.
pub fn validate_binary(name: &str) -> Result<PathBuf, String> {
    let resolved =
        resolve_binary(name).ok_or_else(|| format!("Binary '{}' not found in PATH", name))?;

    let trusted_dirs = build_trusted_dirs();

    let parent = resolved.parent().ok_or_else(|| {
        format!(
            "Cannot determine parent directory of '{}'",
            resolved.display()
        )
    })?;

    // Canonicalize parent for comparison
    let canonical_parent = std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());

    let is_trusted = trusted_dirs.iter().any(|trusted| {
        let canonical_trusted = std::fs::canonicalize(trusted).unwrap_or_else(|_| trusted.clone());
        canonical_parent == canonical_trusted
    });

    if !is_trusted {
        return Err(format!(
            "Binary '{}' resolved to '{}' which is not in a trusted directory",
            name,
            resolved.display()
        ));
    }

    debug!(
        "[SAFE_BINS] Resolved '{}' -> '{}'",
        name,
        resolved.display()
    );
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_trusted_dirs_only_system_dirs() {
        let dirs = build_trusted_dirs();
        assert!(!dirs.is_empty());
        // Should only contain the hardcoded system dirs
        assert_eq!(dirs.len(), TRUSTED_SYSTEM_DIRS.len());
        for dir in &dirs {
            assert!(
                TRUSTED_SYSTEM_DIRS.contains(&dir.to_str().unwrap()),
                "Unexpected trusted dir: {}",
                dir.display()
            );
        }
    }

    #[test]
    fn test_build_trusted_dirs_does_not_include_path_env() {
        // Even if PATH has custom dirs, they should not appear in trusted dirs
        let dirs = build_trusted_dirs();
        let dir_strs: Vec<&str> = dirs.iter().filter_map(|d| d.to_str()).collect();
        // Common non-system PATH entries should not be present
        assert!(!dir_strs.contains(&"/opt/homebrew/bin"));
        assert!(!dir_strs.contains(&"/snap/bin"));
    }

    #[cfg(not(windows))]
    #[test]
    fn test_resolve_binary_sh() {
        let result = resolve_binary("sh");
        assert!(result.is_some(), "sh should be resolvable");
        let path = result.unwrap();
        assert!(path.is_absolute());
    }

    #[cfg(windows)]
    #[test]
    fn test_resolve_binary_cmd() {
        let result = resolve_binary("cmd.exe");
        assert!(result.is_some(), "cmd.exe should be resolvable");
        let path = result.unwrap();
        assert!(path.is_absolute());
    }

    #[test]
    fn test_resolve_binary_nonexistent() {
        let result = resolve_binary("nonexistent_binary_xyz_123");
        assert!(result.is_none());
    }

    #[cfg(not(windows))]
    #[test]
    fn test_resolve_binary_absolute_path() {
        let result = resolve_binary("/bin/sh");
        assert!(result.is_some());
    }

    #[cfg(not(windows))]
    #[test]
    fn test_validate_binary_sh() {
        let result = validate_binary("sh");
        assert!(
            result.is_ok(),
            "sh should be in a trusted directory: {:?}",
            result
        );
    }

    #[cfg(windows)]
    #[test]
    fn test_validate_binary_cmd() {
        let result = validate_binary("cmd.exe");
        assert!(
            result.is_ok(),
            "cmd.exe should be in a trusted directory: {:?}",
            result
        );
    }

    #[test]
    fn test_validate_binary_nonexistent() {
        let result = validate_binary("nonexistent_binary_xyz_123");
        assert!(result.is_err());
    }

    #[test]
    fn test_check_blocked_flags_sort_output() {
        assert!(check_blocked_flags("sort", &["-o", "output.txt"]).is_err());
        assert!(check_blocked_flags("sort", &["--output", "output.txt"]).is_err());
        assert!(check_blocked_flags("sort", &["-r"]).is_ok());
    }

    #[test]
    fn test_check_blocked_flags_jq_file() {
        assert!(check_blocked_flags("jq", &["-f", "filter.jq"]).is_err());
        assert!(check_blocked_flags("jq", &["."]).is_ok());
    }

    #[test]
    fn test_check_blocked_flags_grep_file() {
        assert!(check_blocked_flags("grep", &["-f", "patterns.txt"]).is_err());
        assert!(check_blocked_flags("grep", &["-i", "pattern"]).is_ok());
    }

    #[test]
    fn test_check_blocked_flags_awk_file() {
        assert!(check_blocked_flags("awk", &["-f", "script.awk"]).is_err());
        assert!(check_blocked_flags("awk", &["{print $1}"]).is_ok());
    }

    #[test]
    fn test_check_blocked_flags_unrelated_binary() {
        // Flags for unrelated binaries should not be blocked
        assert!(check_blocked_flags("ls", &["-f"]).is_ok());
        assert!(check_blocked_flags("cat", &["-o"]).is_ok());
    }

    #[test]
    fn test_check_blocked_flags_absolute_path_binary() {
        assert!(check_blocked_flags("/usr/bin/sort", &["-o", "out.txt"]).is_err());
    }
}
