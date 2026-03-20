//! Session recorder: writes asciicast v2 format for later playback.
//!
//! Files are compatible with `asciinema play` and `agg` (GIF conversion).
//!
//! Format:
//! ```
//! {"version": 2, "width": 220, "height": 50, "timestamp": ..., "title": "NexiGate: agent"}
//! [elapsed, "i", "command\n"]
//! [elapsed, "o", "output\n"]
//! ```

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use tracing::{debug, warn};

/// Records terminal I/O in asciicast v2 format.
pub struct SessionRecorder {
    writer: Option<BufWriter<File>>,
    start: Instant,
    enabled: bool,
    #[allow(dead_code)]
    path: PathBuf,
}

impl SessionRecorder {
    /// Create a new recorder.
    ///
    /// If `enabled` is false, all operations are no-ops.
    /// Returns Ok(Self) even on file creation failure (logs warning, disables recording).
    pub fn new(path: PathBuf, agent_id: &str, enabled: bool) -> Result<Self> {
        if !enabled {
            return Ok(Self {
                writer: None,
                start: Instant::now(),
                enabled: false,
                path,
            });
        }

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                warn!("[NEXIGATE/RECORDER] Failed to create recordings dir: {}", e);
                return Ok(Self {
                    writer: None,
                    start: Instant::now(),
                    enabled: false,
                    path,
                });
            }
        }

        let file = match OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
        {
            Ok(f) => f,
            Err(e) => {
                warn!(
                    "[NEXIGATE/RECORDER] Failed to open cast file {:?}: {}",
                    path, e
                );
                return Ok(Self {
                    writer: None,
                    start: Instant::now(),
                    enabled: false,
                    path,
                });
            }
        };

        let mut writer = BufWriter::new(file);

        // Write asciicast v2 header
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let header = serde_json::json!({
            "version": 2,
            "width": 220,
            "height": 50,
            "timestamp": ts,
            "title": format!("NexiGate: {}", agent_id),
            "env": { "TERM": "xterm-256color", "SHELL": crate::config::shell::default_shell_binary() }
        });
        if let Err(e) = writeln!(writer, "{}", header) {
            warn!("[NEXIGATE/RECORDER] Failed to write header: {}", e);
            return Ok(Self {
                writer: None,
                start: Instant::now(),
                enabled: false,
                path,
            });
        }

        debug!("[NEXIGATE/RECORDER] Recording session to {:?}", path);
        Ok(Self {
            writer: Some(writer),
            start: Instant::now(),
            enabled: true,
            path,
        })
    }

    /// Record input data (command typed into shell).
    pub fn record_input(&mut self, data: &str) {
        self.write_event("i", data);
    }

    /// Record output data (shell output to terminal).
    pub fn record_output(&mut self, data: &str) {
        self.write_event("o", data);
    }

    fn write_event(&mut self, event_type: &str, data: &str) {
        if !self.enabled {
            return;
        }
        let elapsed = self.start.elapsed().as_secs_f64();
        let event = serde_json::json!([elapsed, event_type, data]);
        if let Some(ref mut w) = self.writer {
            let _ = writeln!(w, "{}", event);
        }
    }

    /// Flush and close the recording file.
    pub fn finalize(&mut self) {
        if let Some(ref mut w) = self.writer {
            let _ = w.flush();
        }
        self.writer = None;
        self.enabled = false;
    }

    #[allow(dead_code)]
    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    #[allow(dead_code)]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }
}

impl Drop for SessionRecorder {
    fn drop(&mut self) {
        self.finalize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_valid_asciicast_header() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.cast");
        let mut rec = SessionRecorder::new(path.clone(), "test-agent", true).unwrap();
        rec.finalize();

        let content = fs::read_to_string(&path).unwrap();
        let header_line = content.lines().next().unwrap();
        let header: serde_json::Value = serde_json::from_str(header_line).unwrap();
        assert_eq!(header["version"], 2);
        assert_eq!(header["width"], 220);
        assert_eq!(header["height"], 50);
        assert!(header["title"].as_str().unwrap().contains("test-agent"));
    }

    #[test]
    fn test_events_written() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("events.cast");
        let mut rec = SessionRecorder::new(path.clone(), "agent", true).unwrap();
        rec.record_input("ls -la\n");
        rec.record_output("total 0\n");
        rec.finalize();

        let content = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert!(lines.len() >= 3, "header + 2 events");

        let input_line: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(input_line[1], "i");
        assert_eq!(input_line[2], "ls -la\n");

        let output_line: serde_json::Value = serde_json::from_str(lines[2]).unwrap();
        assert_eq!(output_line[1], "o");
        assert_eq!(output_line[2], "total 0\n");
    }

    #[test]
    fn test_timestamps_monotonic() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mono.cast");
        let mut rec = SessionRecorder::new(path.clone(), "agent", true).unwrap();
        rec.record_output("first\n");
        std::thread::sleep(std::time::Duration::from_millis(10));
        rec.record_output("second\n");
        rec.finalize();

        let content = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        let t1: f64 = serde_json::from_str::<serde_json::Value>(lines[1]).unwrap()[0]
            .as_f64()
            .unwrap();
        let t2: f64 = serde_json::from_str::<serde_json::Value>(lines[2]).unwrap()[0]
            .as_f64()
            .unwrap();
        assert!(t2 >= t1, "timestamps must be monotonic");
    }

    #[test]
    fn test_disabled_recorder_noop() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("noop.cast");
        let mut rec = SessionRecorder::new(path.clone(), "agent", false).unwrap();
        rec.record_input("cmd\n");
        rec.record_output("out\n");
        rec.finalize();
        assert!(!path.exists(), "disabled recorder must not create file");
    }

    #[test]
    fn test_finalize_flushes() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("flush.cast");
        let mut rec = SessionRecorder::new(path.clone(), "agent", true).unwrap();
        rec.record_output("data\n");
        rec.finalize();
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("data"), "data should be flushed");
    }
}
