//! Platform detection and platform-specific functionality

use std::process::Command;

/// Augment PATH with common runtime directories that GUI apps miss.
///
/// macOS/Linux apps launched from Finder/Dock only receive /usr/bin:/bin:/usr/sbin:/sbin.
/// This adds Homebrew, nvm, volta, asdf, and ~/.cargo/bin so that `node`, `npm`,
/// and other tools are found regardless of how the app was started.
///
/// On Windows, adds cargo/bin and other common tool paths using USERPROFILE.
#[cfg(not(windows))]
fn augmented_path() -> String {
    let current = std::env::var("PATH").unwrap_or_default();
    let home = std::env::var("HOME").unwrap_or_default();

    let extra: &[&str] = &[
        "/opt/homebrew/bin",
        "/opt/homebrew/sbin",
        "/usr/local/bin",
        "/usr/local/sbin",
    ];

    // Home-relative paths
    let home_extras = if home.is_empty() {
        vec![]
    } else {
        vec![
            format!("{home}/.cargo/bin"),
            format!("{home}/.volta/bin"),
            format!("{home}/.nvm/versions/node/current/bin"),
            format!("{home}/.asdf/shims"),
            format!("{home}/.local/bin"),
        ]
    };

    let mut parts: Vec<&str> = extra.to_vec();
    let home_strs: Vec<String> = home_extras;
    let home_refs: Vec<&str> = home_strs.iter().map(|s| s.as_str()).collect();

    parts.extend(home_refs);
    if !current.is_empty() {
        parts.push(&current);
    }

    parts.join(":")
}

/// Windows version of augmented_path using USERPROFILE and `;` separator.
#[cfg(windows)]
fn augmented_path() -> String {
    let current = std::env::var("PATH").unwrap_or_default();
    let home = std::env::var("USERPROFILE").unwrap_or_default();

    let home_extras = if home.is_empty() {
        vec![]
    } else {
        vec![
            format!("{home}\\.cargo\\bin"),
            format!("{home}\\.volta\\bin"),
            format!("{home}\\AppData\\Roaming\\npm"),
        ]
    };

    let mut parts: Vec<String> = home_extras;
    if !current.is_empty() {
        parts.push(current);
    }

    parts.join(";")
}

/// Create a Command that doesn't flash a console window on Windows and
/// has an augmented PATH so that Homebrew/nvm/volta tools are found on macOS.
pub fn hidden_command(program: &str) -> Command {
    let mut cmd = Command::new(program);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.env("PATH", augmented_path());
    cmd
}

#[cfg(target_os = "macos")]
pub mod macos_bridge;

#[cfg(target_os = "windows")]
pub mod windows_bridge;

#[cfg(target_os = "linux")]
pub mod linux_bridge;

pub mod file_security;

/// Check if running on macOS
#[allow(dead_code)]
pub fn is_macos() -> bool {
    cfg!(target_os = "macos")
}

/// Check if running on Windows
#[allow(dead_code)]
pub fn is_windows() -> bool {
    cfg!(target_os = "windows")
}

/// Check if running on Linux
#[allow(dead_code)]
pub fn is_linux() -> bool {
    cfg!(target_os = "linux")
}

/// Get current platform name
pub fn current_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "unknown"
    }
}

/// Check if macOS Speech framework is available
#[allow(dead_code)]
pub fn has_macos_speech() -> bool {
    #[cfg(target_os = "macos")]
    {
        check_speech_framework()
    }

    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Check if Windows Speech API (SAPI) is available
#[allow(dead_code)]
pub fn has_windows_speech() -> bool {
    cfg!(target_os = "windows")
}

/// Get list of available STT backends for the current platform
pub fn available_stt_backends() -> Vec<&'static str> {
    let mut backends = Vec::new();

    #[cfg(target_os = "macos")]
    backends.push("macos_speech");

    #[cfg(target_os = "windows")]
    backends.push("windows_speech");

    // SenseVoice is available on all platforms if model is present
    backends.push("sensevoice");

    // Cloud backends always listed (availability depends on API keys)
    backends.push("deepgram");
    backends.push("openai");

    backends
}

/// Get list of available TTS backends for the current platform
pub fn available_tts_backends() -> Vec<&'static str> {
    let mut backends = Vec::new();

    #[cfg(target_os = "macos")]
    backends.push("macos_say");

    #[cfg(target_os = "windows")]
    backends.push("windows_sapi");

    // Piper available on all platforms if binary is on PATH
    backends.push("piper");

    #[cfg(target_os = "linux")]
    backends.push("espeak");

    // Cloud backends always listed
    backends.push("elevenlabs");
    backends.push("cartesia");

    backends
}

/// Return the default script shell for the current platform.
///
/// Windows: `%COMSPEC%` or `cmd.exe`; Unix: `/bin/sh`
pub fn default_script_shell() -> std::path::PathBuf {
    #[cfg(windows)]
    {
        std::env::var("COMSPEC")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::path::PathBuf::from("cmd.exe"))
    }
    #[cfg(not(windows))]
    {
        std::path::PathBuf::from("/bin/sh")
    }
}

/// Return the shell execution flag for running a script string.
///
/// Windows (cmd.exe): `/C`; Unix (sh): `-c`
#[allow(dead_code)]
pub fn script_shell_exec_flag() -> &'static str {
    #[cfg(windows)]
    {
        "/C"
    }
    #[cfg(not(windows))]
    {
        "-c"
    }
}

/// Return the default path for the K2K agent binary.
///
/// Windows: checks `%CARGO_HOME%\bin\knowledge-nexus-agent.exe` then `Program Files`.
/// macOS/Linux: `/usr/local/bin/knowledge-nexus-agent`
pub fn default_k2k_agent_binary() -> String {
    #[cfg(windows)]
    {
        // Check CARGO_HOME first
        if let Ok(cargo_home) = std::env::var("CARGO_HOME") {
            let cargo_bin = std::path::PathBuf::from(&cargo_home)
                .join("bin")
                .join("knowledge-nexus-agent.exe");
            if cargo_bin.exists() {
                return cargo_bin.to_string_lossy().to_string();
            }
        }
        // Check Program Files
        if let Ok(pf) = std::env::var("ProgramFiles") {
            let pf_bin = std::path::PathBuf::from(&pf)
                .join("NexiBot")
                .join("knowledge-nexus-agent.exe");
            if pf_bin.exists() {
                return pf_bin.to_string_lossy().to_string();
            }
        }
        "knowledge-nexus-agent.exe".to_string()
    }
    #[cfg(not(windows))]
    {
        "/usr/local/bin/knowledge-nexus-agent".to_string()
    }
}

/// Open a URL in the default system browser
pub fn open_browser(url: &str) -> anyhow::Result<()> {
    tracing::info!("[BROWSER] Opening URL: {}", url);
    open::that(url).map_err(|e| {
        tracing::error!("[BROWSER] Failed to open URL: {}", e);
        anyhow::anyhow!("Failed to open browser: {}", e)
    })
}

/// Check if macOS version supports Speech framework (10.15+)
#[cfg(target_os = "macos")]
fn check_speech_framework() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_platform_detection() {
        #[cfg(target_os = "macos")]
        {
            assert!(is_macos());
            assert!(!is_windows());
            assert!(!is_linux());
            assert_eq!(current_platform(), "macos");
        }

        #[cfg(target_os = "windows")]
        {
            assert!(!is_macos());
            assert!(is_windows());
            assert!(!is_linux());
            assert_eq!(current_platform(), "windows");
        }

        #[cfg(target_os = "linux")]
        {
            assert!(!is_macos());
            assert!(!is_windows());
            assert!(is_linux());
            assert_eq!(current_platform(), "linux");
        }
    }

    #[test]
    fn test_available_backends() {
        let stt = available_stt_backends();
        let tts = available_tts_backends();

        // Cloud backends should always be present
        assert!(stt.contains(&"deepgram"));
        assert!(stt.contains(&"openai"));
        assert!(stt.contains(&"sensevoice"));
        assert!(tts.contains(&"elevenlabs"));
        assert!(tts.contains(&"cartesia"));
        assert!(tts.contains(&"piper"));
    }
}
