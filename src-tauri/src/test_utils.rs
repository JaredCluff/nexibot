//! Cross-platform test helpers.
//!
//! Provides platform-aware command strings and shell paths for use in tests,
//! so tests don't hardcode Unix assumptions.
//!
//! On Windows, the default shell is PowerShell (`powershell.exe`), so all
//! commands here use PowerShell syntax (NOT CMD/batch syntax).

/// Return the platform default shell binary for tests.
pub fn test_shell_binary() -> String {
    crate::config::shell::default_shell_binary()
}

/// Return a command that always exits with a non-zero status.
///
/// On Windows/PowerShell we run `cmd /c "exit 1"` which sets `$LASTEXITCODE`
/// to 1 without killing the PowerShell session itself.
pub fn cmd_exit_fail() -> &'static str {
    #[cfg(windows)]
    {
        "cmd /c \"exit 1\""
    }
    #[cfg(not(windows))]
    {
        "false"
    }
}

/// Return a command that always exits with status 0.
///
/// On Windows/PowerShell we run `cmd /c "exit 0"` which sets `$LASTEXITCODE`
/// to 0.
pub fn cmd_exit_ok() -> &'static str {
    #[cfg(windows)]
    {
        "cmd /c \"exit 0\""
    }
    #[cfg(not(windows))]
    {
        "true"
    }
}

/// Return a shell command that echoes the value of an environment variable.
pub fn cmd_echo_var(name: &str) -> String {
    #[cfg(windows)]
    {
        format!("Write-Host $env:{}", name)
    }
    #[cfg(not(windows))]
    {
        format!("echo ${}", name)
    }
}

/// Return a command that sets an environment variable.
pub fn cmd_set_var(name: &str, value: &str) -> String {
    #[cfg(windows)]
    {
        format!("$env:{} = '{}'", name, value)
    }
    #[cfg(not(windows))]
    {
        format!("export {}={}", name, value)
    }
}

/// Return a command that sleeps for a long time (for timeout tests).
pub fn cmd_sleep_long() -> &'static str {
    #[cfg(windows)]
    {
        "Start-Sleep -Seconds 60"
    }
    #[cfg(not(windows))]
    {
        "sleep 60"
    }
}

/// Return a command that prints multiple lines.
pub fn cmd_multiline_output() -> &'static str {
    #[cfg(windows)]
    {
        "Write-Host 'line1'; Write-Host 'line2'; Write-Host 'line3'"
    }
    #[cfg(not(windows))]
    {
        r#"printf "line1\nline2\nline3\n""#
    }
}

/// Return a command that writes a message to stderr.
pub fn cmd_write_stderr(msg: &str) -> String {
    #[cfg(windows)]
    {
        format!("[Console]::Error.WriteLine('{}')", msg)
    }
    #[cfg(not(windows))]
    {
        format!("echo '{}' >&2", msg)
    }
}

/// Return a command that generates ~200 numbered lines of output.
pub fn cmd_large_output_200_lines() -> &'static str {
    #[cfg(windows)]
    {
        "1..200 | ForEach-Object { 'line {0:D4}: AABBCCDD0011223344' -f $_ }"
    }
    #[cfg(not(windows))]
    {
        "for i in $(seq 1 200); do printf 'line %04d: AABBCCDD0011223344\\n' $i; done"
    }
}

/// Return a command that prints text without a trailing newline.
pub fn cmd_printf_no_newline(text: &str) -> String {
    #[cfg(windows)]
    {
        format!("Write-Host '{}' -NoNewline", text)
    }
    #[cfg(not(windows))]
    {
        format!("printf '{}'", text)
    }
}

/// Return the `printenv` equivalent command for capturing all environment variables.
///
/// Output format: `KEY=VALUE` per line (matching `printenv` output on Unix).
pub fn cmd_printenv() -> &'static str {
    #[cfg(windows)]
    {
        "Get-ChildItem env: | ForEach-Object { \"$($_.Name)=$($_.Value)\" }"
    }
    #[cfg(not(windows))]
    {
        "printenv"
    }
}

/// Return a temporary directory path string for cross-platform test use.
pub fn temp_dir_string() -> String {
    std::env::temp_dir().to_string_lossy().to_string()
}
