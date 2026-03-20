//! Cross-platform file and directory permission management.
//!
//! On Unix: uses `chmod` via `std::os::unix::fs::PermissionsExt`.
//! On Windows: uses `icacls` to restrict ACLs to the current user.

use std::path::Path;
use tracing::debug;

/// Restrict a file to owner-only read/write (Unix: 0o600, Windows: icacls current user only).
pub fn restrict_file_permissions(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| anyhow::anyhow!("Failed to set file permissions on {:?}: {}", path, e))?;
        debug!("[FILE_SECURITY] Set {:?} to mode 0600", path);
    }

    #[cfg(windows)]
    {
        restrict_windows_acl(path, false)?;
    }

    Ok(())
}

/// Restrict a directory to owner-only access (Unix: 0o700, Windows: icacls with inheritance).
pub fn restrict_dir_permissions(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| anyhow::anyhow!("Failed to set dir permissions on {:?}: {}", path, e))?;
        debug!("[FILE_SECURITY] Set {:?} to mode 0700", path);
    }

    #[cfg(windows)]
    {
        restrict_windows_acl(path, true)?;
    }

    Ok(())
}

/// Check file/directory permissions and return a warning message if too permissive.
///
/// Returns `Some(warning)` if permissions are too broad, `None` if acceptable.
pub fn check_file_permissions(path: &Path) -> Option<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match std::fs::metadata(path) {
            Ok(meta) => {
                let mode = meta.mode() & 0o777;
                if mode > 0o600 {
                    return Some(format!(
                        "{} has mode {:04o}, expected 0600 or tighter",
                        path.display(),
                        mode
                    ));
                }
            }
            Err(e) => {
                debug!("[FILE_SECURITY] Could not stat {:?}: {}", path, e);
            }
        }
    }

    #[cfg(windows)]
    {
        if let Some(warning) = check_windows_acl(path) {
            return Some(warning);
        }
    }

    None
}

/// Check directory permissions and return a warning if too permissive.
pub fn check_dir_permissions(path: &Path) -> Option<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match std::fs::metadata(path) {
            Ok(meta) => {
                let mode = meta.mode() & 0o777;
                if mode > 0o700 {
                    return Some(format!(
                        "{} has mode {:04o}, expected 0700 or tighter",
                        path.display(),
                        mode
                    ));
                }
            }
            Err(e) => {
                debug!("[FILE_SECURITY] Could not stat {:?}: {}", path, e);
            }
        }
    }

    #[cfg(windows)]
    {
        if let Some(warning) = check_windows_acl(path) {
            return Some(warning);
        }
    }

    None
}

/// Return a platform-appropriate fix hint for file permission issues.
pub fn fix_hint_for_file(path: &Path) -> String {
    #[cfg(unix)]
    {
        format!("Run: chmod 600 {}", path.display())
    }
    #[cfg(windows)]
    {
        format!(
            "Run: icacls \"{}\" /inheritance:r /grant:r \"%USERNAME%:(R,W)\"",
            path.display()
        )
    }
}

/// Return a platform-appropriate fix hint for directory permission issues.
pub fn fix_hint_for_dir(path: &Path) -> String {
    #[cfg(unix)]
    {
        format!("Run: chmod 700 {}", path.display())
    }
    #[cfg(windows)]
    {
        format!(
            "Run: icacls \"{}\" /inheritance:r /grant:r \"%USERNAME%:(OI)(CI)(F)\"",
            path.display()
        )
    }
}

// ---------------------------------------------------------------------------
// Windows-specific helpers
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn restrict_windows_acl(path: &Path, is_dir: bool) -> anyhow::Result<()> {
    let username = std::env::var("USERNAME").unwrap_or_else(|_| "".to_string());
    if username.is_empty() {
        warn!("[FILE_SECURITY] Could not determine USERNAME; skipping ACL restriction");
        return Ok(());
    }

    let path_str = path.to_string_lossy();

    // Remove inherited ACEs
    let status = super::hidden_command("icacls")
        .args([&*path_str, "/inheritance:r"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    match status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            warn!(
                "[FILE_SECURITY] icacls /inheritance:r exited with {}",
                s.code().unwrap_or(-1)
            );
        }
        Err(e) => {
            return Err(anyhow::anyhow!(
                "Failed to run icacls on {:?}: {}",
                path,
                e
            ));
        }
    }

    // Grant current user appropriate permissions
    let grant = if is_dir {
        format!("{}:(OI)(CI)(F)", username)
    } else {
        format!("{}:(R,W)", username)
    };

    let status = super::hidden_command("icacls")
        .args([&*path_str, "/grant:r", &grant])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    match status {
        Ok(s) if s.success() => {
            debug!("[FILE_SECURITY] Restricted ACLs on {:?} to {}", path, username);
        }
        Ok(s) => {
            warn!(
                "[FILE_SECURITY] icacls /grant:r exited with {}",
                s.code().unwrap_or(-1)
            );
        }
        Err(e) => {
            return Err(anyhow::anyhow!(
                "Failed to run icacls grant on {:?}: {}",
                path,
                e
            ));
        }
    }

    Ok(())
}

#[cfg(windows)]
fn check_windows_acl(path: &Path) -> Option<String> {
    let path_str = path.to_string_lossy();
    let output = super::hidden_command("icacls")
        .arg(&*path_str)
        .output();

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            // Check for overly broad ACEs
            let broad_principals = ["Everyone", "BUILTIN\\Users", "Authenticated Users"];
            for principal in &broad_principals {
                if stdout.contains(principal) {
                    return Some(format!(
                        "{} has a broad ACE for '{}'. Restrict to current user only.",
                        path.display(),
                        principal
                    ));
                }
            }
        }
        Err(e) => {
            debug!("[FILE_SECURITY] Could not run icacls on {:?}: {}", path, e);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_restrict_file_permissions_temp() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let result = restrict_file_permissions(tmp.path());
        assert!(result.is_ok(), "restrict_file_permissions failed: {:?}", result);
    }

    #[test]
    fn test_restrict_dir_permissions_temp() {
        let tmp = tempfile::tempdir().unwrap();
        let result = restrict_dir_permissions(tmp.path());
        assert!(result.is_ok(), "restrict_dir_permissions failed: {:?}", result);
    }

    #[test]
    fn test_check_file_permissions_after_restrict() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        restrict_file_permissions(tmp.path()).unwrap();
        let warning = check_file_permissions(tmp.path());
        // After restricting, should be no warning
        assert!(warning.is_none(), "Unexpected warning: {:?}", warning);
    }

    #[test]
    fn test_fix_hints_not_empty() {
        let p = std::path::Path::new("/tmp/test");
        assert!(!fix_hint_for_file(p).is_empty());
        assert!(!fix_hint_for_dir(p).is_empty());
    }
}
