//! Path traversal prevention.
//!
//! Validates paths to prevent directory traversal attacks and symlink escapes.

use std::path::{Component, Path, PathBuf};
use tracing::debug;

/// Maximum directory depth allowed in a path.
const MAX_DEPTH: usize = 32;

/// Validate that a path stays within a root directory.
///
/// Performs a dual check: first normalizes the path lexically (without
/// touching the filesystem), then canonicalizes it (resolving symlinks)
/// if it exists. Returns the validated canonical path.
///
/// # TOCTOU (Time-of-Check / Time-of-Use) Warning
///
/// This function validates the path at the moment it is called, but the
/// returned `PathBuf` is **not** an open file descriptor.  A window exists
/// between the validation performed here and the moment the caller actually
/// opens the file during which an attacker with filesystem access could:
///
/// 1. Replace a legitimate directory component with a symlink pointing
///    outside the root.
/// 2. Atomically swap the validated path for a different file.
///
/// **Callers MUST open the file immediately after this function returns,
/// with no intervening filesystem operations that could introduce a race.**
/// For sensitive paths, additionally call [`validate_path_no_symlink_components`]
/// before this function: that check rejects symlink components at validation
/// time, making the swap attack significantly harder to execute.
///
/// # See also
/// [`validate_path_no_symlink_components`] — rejects paths where any existing
/// component is a symlink, hardening against TOCTOU symlink-swap attacks.
pub fn validate_path_within(path: &Path, root: &Path) -> Result<PathBuf, String> {
    let path_str = path.to_string_lossy();

    // Null byte detection
    if path_str.contains('\0') {
        return Err("Path contains null bytes".to_string());
    }

    // Reject any path containing `..` components — this is a strict security check
    for component in path.components() {
        if matches!(component, Component::ParentDir) {
            return Err(format!(
                "Path '{}' contains '..' traversal component",
                path.display()
            ));
        }
    }

    // Normalize the path lexically (just resolves `.`)
    let normalized = normalize_path(path);

    // Check depth
    let depth = normalized.components().count();
    if depth > MAX_DEPTH {
        return Err(format!("Path exceeds maximum depth of {}", MAX_DEPTH));
    }

    // Normalize root for comparison
    let normalized_root = if root.exists() {
        root.canonicalize()
            .map_err(|e| format!("Cannot canonicalize root: {}", e))?
    } else {
        normalize_path(root)
    };

    // Lexical check: normalized path must start with root
    let abs_path = if normalized.is_absolute() {
        normalized.clone()
    } else {
        normalized_root.join(&normalized)
    };

    // Component-aware prefix check (handles both `/` and `\` separators)
    if !abs_path.starts_with(&normalized_root) {
        return Err(format!(
            "Path '{}' escapes root directory '{}'",
            path.display(),
            root.display()
        ));
    }

    // Filesystem check: if the path exists, canonicalize and re-verify
    if abs_path.exists() {
        let canonical = abs_path
            .canonicalize()
            .map_err(|e| format!("Cannot canonicalize path: {}", e))?;

        if !canonical.starts_with(&normalized_root) {
            return Err(format!(
                "Path '{}' escapes root via symlink (resolves to '{}')",
                path.display(),
                canonical.display()
            ));
        }

        debug!(
            "[PATH_VALIDATE] Validated: {} -> {}",
            path.display(),
            canonical.display()
        );
        return Ok(canonical);
    }

    debug!(
        "[PATH_VALIDATE] Validated (non-existent): {} -> {}",
        path.display(),
        abs_path.display()
    );
    Ok(abs_path)
}

/// Validate a path against the NexiBot config directory.
pub fn validate_config_path(path: &Path) -> Result<PathBuf, String> {
    let config_dir = directories::ProjectDirs::from("ai", "nexibot", "desktop")
        .ok_or_else(|| "Failed to get project directories".to_string())?
        .config_dir()
        .to_path_buf();

    validate_path_within(path, &config_dir)
}

/// Normalize a path by resolving `.` and `..` components lexically.
fn normalize_path(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();

    for component in path.components() {
        match component {
            Component::ParentDir => {
                result.pop();
            }
            Component::CurDir => {}
            other => {
                result.push(other);
            }
        }
    }

    result
}

/// Validate that a filename is safe for use in a skill directory.
///
/// Rejects path separators, traversal sequences, null bytes, and symlink-like names.
/// Returns the validated filename or an error.
pub fn validate_skill_filename(filename: &str) -> Result<String, String> {
    if filename.is_empty() {
        return Err("Empty filename".to_string());
    }

    // Reject null bytes
    if filename.contains('\0') {
        return Err(format!("Filename '{}' contains null bytes", filename));
    }

    // Reject Unicode bidirectional control characters.
    // These characters (e.g., U+202E RIGHT-TO-LEFT OVERRIDE) can visually disguise
    // a malicious file extension when displayed in terminals or file managers.
    for ch in filename.chars() {
        let cp = ch as u32;
        if matches!(cp, 0x200E | 0x200F | 0x202A..=0x202E | 0x2066..=0x2069 | 0xFFF9..=0xFFFB) {
            return Err(format!(
                "Filename '{}' contains bidirectional or interlinear annotation control characters",
                filename
            ));
        }
    }

    // Reject path separators (both Unix and Windows)
    if filename.contains('/') || filename.contains('\\') {
        return Err(format!(
            "Filename '{}' contains path separators (potential Zip Slip attack)",
            filename
        ));
    }

    // Reject traversal components
    if filename == ".." || filename == "." || filename.starts_with("..") {
        return Err(format!(
            "Filename '{}' contains traversal component",
            filename
        ));
    }

    // Reject hidden files starting with '.' (common attack vector)
    // except for well-known config files
    if filename.starts_with('.') && filename != ".gitkeep" {
        return Err(format!(
            "Filename '{}' is a hidden file (potential attack vector)",
            filename
        ));
    }

    Ok(filename.to_string())
}

/// Validate that no component in the path is a symlink.
/// This prevents TOCTOU attacks where a directory component is a symlink
/// that gets swapped between validation and use.
pub fn validate_path_no_symlink_components(path: &Path) -> Result<(), String> {
    let mut current = std::path::PathBuf::new();
    for component in path.components() {
        current.push(component);
        if current.exists() {
            let meta = std::fs::symlink_metadata(&current)
                .map_err(|e| format!("Failed to stat '{}': {}", current.display(), e))?;
            if meta.file_type().is_symlink() {
                return Err(format!(
                    "Path component '{}' is a symlink (symlink traversal blocked)",
                    current.display()
                ));
            }
        }
    }
    Ok(())
}

/// Validate that a path is not a symlink.
pub fn reject_symlink(path: &Path) -> Result<(), String> {
    if path.exists() {
        let metadata = std::fs::symlink_metadata(path)
            .map_err(|e| format!("Cannot read metadata for '{}': {}", path.display(), e))?;
        if metadata.is_symlink() {
            return Err(format!(
                "Path '{}' is a symlink (rejected for security)",
                path.display()
            ));
        }
    }
    Ok(())
}

/// Validate that a plugin/hook path is safe to load.
///
/// Blocks:
/// - Paths that escape the expected directory (root escapes)
/// - World-writable paths (on Unix)
/// - Symlinks pointing outside the expected directory
#[allow(dead_code)]
pub fn validate_plugin_path(path: &Path, expected_parent: &Path) -> Result<(), String> {
    // Check symlink
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() {
            // Resolve the symlink and check it's within expected_parent
            if let Ok(resolved) = std::fs::canonicalize(path) {
                if let Ok(parent_canon) = std::fs::canonicalize(expected_parent) {
                    if !resolved.starts_with(&parent_canon) {
                        return Err(format!(
                            "Plugin path {:?} is a symlink pointing outside {:?}",
                            path, expected_parent
                        ));
                    }
                }
            }
        }

        // Check world-writable on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = metadata.permissions().mode();
            if mode & 0o002 != 0 {
                return Err(format!(
                    "Plugin path {:?} is world-writable (mode {:o})",
                    path, mode
                ));
            }
        }

        // Check for overly broad ACLs on Windows
        #[cfg(windows)]
        {
            if let Some(warning) = crate::platform::file_security::check_file_permissions(path) {
                return Err(format!("Plugin path {:?}: {}", path, warning));
            }
        }
    }

    // Check path containment
    if let (Ok(resolved), Ok(parent_canon)) = (
        std::fs::canonicalize(path),
        std::fs::canonicalize(expected_parent),
    ) {
        if !resolved.starts_with(&parent_canon) {
            return Err(format!(
                "Plugin path {:?} escapes expected directory {:?}",
                path, expected_parent
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_normalize_path() {
        // normalize_path still handles .. for internal use,
        // but validate_path_within rejects .. before calling it
        assert_eq!(
            normalize_path(Path::new("/a/b/../c")),
            PathBuf::from("/a/c")
        );
        assert_eq!(
            normalize_path(Path::new("/a/./b/./c")),
            PathBuf::from("/a/b/c")
        );
    }

    #[test]
    fn test_validate_path_within_success() {
        let tmp_dir = std::env::temp_dir();
        let root = std::fs::canonicalize(&tmp_dir).unwrap_or(tmp_dir);
        let path = root.join("test/file.txt");
        assert!(validate_path_within(&path, &root).is_ok());
    }

    #[test]
    fn test_validate_path_within_traversal() {
        let tmp_dir = std::env::temp_dir();
        let root = std::fs::canonicalize(&tmp_dir).unwrap_or(tmp_dir);
        let safe = root.join("safe");
        let path = PathBuf::from("../../etc/passwd");
        let result = validate_path_within(&path, &safe);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains(".."));
    }

    #[test]
    fn test_null_bytes_rejected() {
        let root = std::env::temp_dir();
        let path = root.join("file\0.txt");
        assert!(validate_path_within(&path, &root).is_err());
    }

    #[test]
    fn test_depth_limit() {
        let tmp_dir = std::env::temp_dir();
        let root = std::fs::canonicalize(&tmp_dir).unwrap_or(tmp_dir);
        let deep_suffix = (0..35)
            .map(|i| format!("d{}", i))
            .collect::<Vec<_>>()
            .join("/");
        let path = root.join(deep_suffix);
        assert!(validate_path_within(&path, &root).is_err());
    }

    // --- Skill filename validation tests ---

    #[test]
    fn test_valid_skill_filename() {
        assert!(validate_skill_filename("script.sh").is_ok());
        assert!(validate_skill_filename("my-tool.py").is_ok());
        assert!(validate_skill_filename("README.md").is_ok());
    }

    #[test]
    fn test_zip_slip_filename() {
        assert!(validate_skill_filename("../../etc/passwd").is_err());
        assert!(validate_skill_filename("../evil.sh").is_err());
        assert!(validate_skill_filename("subdir/script.sh").is_err());
        assert!(validate_skill_filename("sub\\dir\\script.sh").is_err());
    }

    #[test]
    fn test_hidden_file_rejected() {
        assert!(validate_skill_filename(".evil").is_err());
        assert!(validate_skill_filename(".bashrc").is_err());
        // .gitkeep is allowed
        assert!(validate_skill_filename(".gitkeep").is_ok());
    }

    #[test]
    fn test_empty_filename_rejected() {
        assert!(validate_skill_filename("").is_err());
    }

    #[test]
    fn test_null_byte_filename_rejected() {
        assert!(validate_skill_filename("file\0.sh").is_err());
    }

    // --- Plugin path validation tests ---

    #[test]
    fn test_validate_plugin_path_within_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let parent = tmp.path();
        let plugin = parent.join("my-plugin.so");
        std::fs::write(&plugin, b"fake plugin").unwrap();
        assert!(validate_plugin_path(&plugin, parent).is_ok());
    }

    #[test]
    fn test_validate_plugin_path_outside_parent() {
        let tmp1 = tempfile::tempdir().unwrap();
        let tmp2 = tempfile::tempdir().unwrap();
        let plugin = tmp2.path().join("evil.so");
        std::fs::write(&plugin, b"evil").unwrap();
        // Plugin is in tmp2, but expected_parent is tmp1 — should fail
        let result = validate_plugin_path(&plugin, tmp1.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("escapes expected directory"));
    }

    #[cfg(unix)]
    #[test]
    fn test_validate_plugin_path_world_writable() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let plugin = tmp.path().join("writable.so");
        std::fs::write(&plugin, b"data").unwrap();
        std::fs::set_permissions(&plugin, std::fs::Permissions::from_mode(0o777)).unwrap();
        let result = validate_plugin_path(&plugin, tmp.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("world-writable"));
    }

    #[cfg(unix)]
    #[test]
    fn test_validate_plugin_path_symlink_escape() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_file = outside.path().join("secret.so");
        std::fs::write(&outside_file, b"secret").unwrap();

        let symlink_path = tmp.path().join("link.so");
        std::os::unix::fs::symlink(&outside_file, &symlink_path).unwrap();

        let result = validate_plugin_path(&symlink_path, tmp.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("symlink pointing outside"));
    }

    // --- Symlink component validation tests ---

    #[test]
    fn test_validate_path_no_symlink_components_regular_path() {
        let tmp = tempfile::tempdir().unwrap();
        // Use canonicalized path to avoid macOS symlinks (/var -> /private/var)
        let canonical_tmp = tmp.path().canonicalize().unwrap();
        let subdir = canonical_tmp.join("subdir");
        std::fs::create_dir(&subdir).unwrap();
        let file = subdir.join("file.txt");
        std::fs::write(&file, b"data").unwrap();

        assert!(validate_path_no_symlink_components(&file).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn test_validate_path_no_symlink_components_detects_symlink_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let real_dir = tmp.path().join("real");
        std::fs::create_dir(&real_dir).unwrap();
        std::fs::write(real_dir.join("secret.txt"), b"secret").unwrap();

        let link_dir = tmp.path().join("link");
        std::os::unix::fs::symlink(&real_dir, &link_dir).unwrap();

        let path_through_symlink = link_dir.join("secret.txt");
        let result = validate_path_no_symlink_components(&path_through_symlink);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("symlink traversal blocked"));
    }

    #[test]
    fn test_validate_path_no_symlink_components_nonexistent_ok() {
        // Non-existent components are allowed (they can't be symlinks)
        let tmp = std::env::temp_dir();
        let real_tmp = std::fs::canonicalize(&tmp).unwrap_or(tmp);
        let path = real_tmp.join("nonexistent_dir_xyz/file.txt");
        assert!(validate_path_no_symlink_components(&path).is_ok());
    }
}
