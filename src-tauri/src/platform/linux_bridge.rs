//! Linux platform bridge
//!
//! Provides helper functions for detecting available speech tools on Linux.

use std::process::Command;

/// Check if espeak-ng is available on PATH
pub fn has_espeak() -> bool {
    Command::new("espeak-ng")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Check if Piper TTS is available on PATH
pub fn has_piper() -> bool {
    Command::new("piper")
        .arg("--help")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
