//! ShellSession: persistent bash session via PTY with sentinel-based command execution.
//!
//! Each session maintains a long-lived bash process. Commands are written to the PTY
//! master, and output is read until a unique sentinel marker appears, signaling completion.
//!
//! The PTY reader runs in a dedicated OS thread and forwards lines to a tokio channel,
//! allowing async `run_command` calls with timeout support.

use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, Result};
use portable_pty::{Child as PtyChild, CommandBuilder, NativePtySystem, PtySize, PtySystem};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::debug;
use uuid::Uuid;

use super::audit::AuditLog;
use super::recorder::SessionRecorder;

/// Public metadata about a shell session (safe to serialize).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellSessionInfo {
    pub session_id: String,
    pub agent_id: String,
    pub created_at_ms: u64,
    pub last_used_ms: u64,
    pub commands_run: u64,
}

/// A persistent bash session backed by a PTY.
pub struct ShellSession {
    pub session_id: String,
    pub agent_id: String,
    /// Sync write handle to PTY master (cheap per-write lock).
    master_writer: Arc<StdMutex<Box<dyn Write + Send>>>,
    /// Lines arriving from the PTY reader thread.
    output_rx: mpsc::UnboundedReceiver<String>,
    /// Monotonically increasing sentinel sequence number.
    sentinel_seq: AtomicU64,
    /// Sentinel prefix: "__NEXIGATE__" by default.
    sentinel_prefix: String,
    /// Optional session recorder (asciicast v2).
    pub recorder: Option<SessionRecorder>,
    /// Per-session audit log.
    pub audit_log: AuditLog,
    pub created_at: SystemTime,
    pub last_used: SystemTime,
    pub commands_run: u64,
    /// Last captured environment snapshot for env-diff discovery.
    /// None until the first `snapshot_env()` call.
    pub last_env_snapshot: Option<HashMap<String, String>>,
    /// Child process handle — kept alive so we can kill and reap on drop.
    child: StdMutex<Box<dyn PtyChild + Send + Sync>>,
}

impl ShellSession {
    /// Spawn a new bash session and wait for it to be ready.
    ///
    /// `env_map`: (key, value) pairs exported into the bash environment at startup.
    pub async fn new(
        agent_id: &str,
        shell_binary: &str,
        sentinel_prefix: &str,
        env_map: &[(String, String)],
        recorder: Option<SessionRecorder>,
        max_audit_entries: usize,
    ) -> Result<Self> {
        let session_id = Uuid::new_v4().to_string();

        // Append per-session random bytes to the sentinel prefix so a model cannot
        // predict or reproduce the sentinel marker in its command output.
        let mut rand_bytes = [0u8; 8];
        rand::rngs::OsRng.fill_bytes(&mut rand_bytes);
        let session_rand: String = rand_bytes.iter().map(|b| format!("{:02x}", b)).collect();
        let sentinel_prefix = format!("{}{}__", sentinel_prefix, session_rand);
        let sentinel_prefix = sentinel_prefix.as_str();

        // Open PTY
        let pty_system = NativePtySystem::default();
        let pair = pty_system
            .openpty(PtySize {
                rows: 50,
                cols: 220,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| anyhow!("Failed to open PTY: {}", e))?;

        // Build shell command with platform-appropriate flags
        let mut cmd = CommandBuilder::new(shell_binary);
        #[cfg(windows)]
        {
            cmd.arg("-NoProfile");
            cmd.arg("-NoLogo");
        }
        #[cfg(not(windows))]
        {
            cmd.arg("--norc");
            cmd.arg("--noprofile");
        }
        // Clear the inherited environment so sensitive env vars from the parent process
        // (API keys, tokens, secrets) cannot leak into the bash session.
        // Only the explicitly provided env_map entries are exported.
        cmd.env_clear();
        for (k, v) in env_map {
            cmd.env(k, v);
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| anyhow!("Failed to spawn shell: {}", e))?;

        // Close slave end so bash EOF propagates correctly when master is closed
        drop(pair.slave);

        let master = pair.master;
        let writer = master
            .take_writer()
            .map_err(|e| anyhow!("Failed to get PTY writer: {}", e))?;
        let reader = master
            .try_clone_reader()
            .map_err(|e| anyhow!("Failed to get PTY reader: {}", e))?;

        // Spawn OS thread for blocking PTY reads → tokio channel
        let (tx, mut rx) = mpsc::channel::<String>(256);
        let tx_clone = tx;
        std::thread::spawn(move || {
            use std::io::BufRead;
            let mut buf = std::io::BufReader::new(reader);
            let mut line = String::new();
            loop {
                line.clear();
                match buf.read_line(&mut line) {
                    Ok(0) => {
                        debug!("[NEXIGATE/SESSION] PTY reader EOF");
                        break;
                    }
                    Ok(_) => {
                        if tx_clone.blocking_send(line.clone()).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        debug!("[NEXIGATE/SESSION] PTY read error: {}", e);
                        break;
                    }
                }
            }
        });

        let writer_arc = Arc::new(StdMutex::new(writer));

        // Send init commands + init sentinel to drain startup noise
        let init_sentinel = format!("{}INIT", sentinel_prefix);
        {
            let mut w = writer_arc
                .lock()
                .map_err(|_| anyhow!("Writer lock poisoned"))?;
            // PowerShell: suppress prompt and echo sentinel
            #[cfg(windows)]
            let init = format!(
                "function prompt {{ }} ; Write-Host '{}'\r\n",
                init_sentinel
            );
            // Bash/sh: suppress prompt noise and echo sentinel
            #[cfg(not(windows))]
            let init = format!(
                "stty -echo 2>/dev/null; unset PS1 PS2 PS3 PS4 HISTFILE 2>/dev/null; \
                 export PS1='' PS2='' PS3='' PS4='' HISTFILE=/dev/null\n\
                 printf '{}\\n'\n",
                init_sentinel
            );
            w.write_all(init.as_bytes())
                .map_err(|e| anyhow!("Failed to write init: {}", e))?;
            w.flush().map_err(|e| anyhow!("PTY flush failed: {}", e))?;
        }

        // Drain until init sentinel (timeout 10s)
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                debug!("[NEXIGATE/SESSION] Init sentinel drain timed out; proceeding anyway");
                break;
            }
            match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Some(line)) => {
                    if line.trim_end_matches('\n').trim_end_matches('\r') == init_sentinel {
                        debug!("[NEXIGATE/SESSION] Session {} ready", &session_id[..8]);
                        break;
                    }
                }
                Ok(None) => return Err(anyhow!("Shell exited during init")),
                Err(_) => {
                    debug!("[NEXIGATE/SESSION] Init timed out; proceeding anyway");
                    break;
                }
            }
        }

        Ok(Self {
            session_id,
            agent_id: agent_id.to_string(),
            master_writer: writer_arc,
            output_rx: rx,
            sentinel_seq: AtomicU64::new(1),
            sentinel_prefix: sentinel_prefix.to_string(),
            recorder,
            audit_log: AuditLog::new(max_audit_entries),
            created_at: SystemTime::now(),
            last_used: SystemTime::now(),
            commands_run: 0,
            last_env_snapshot: None,
            child: StdMutex::new(child),
        })
    }

    /// Execute a command in this bash session.
    ///
    /// Returns `(output, exit_code)` where output is everything printed before
    /// the sentinel line (stdout and stderr merged by the PTY).
    pub async fn run_command(&mut self, command: &str, timeout: Duration) -> Result<(String, i32)> {
        let seq = self.sentinel_seq.fetch_add(1, Ordering::Relaxed);
        let sentinel = format!("{}{:016x}", self.sentinel_prefix, seq);

        // Write command followed by sentinel (cross-platform)
        // PowerShell: $LASTEXITCODE holds the exit code of the last native command.
        // Default to 0 if $LASTEXITCODE is $null (e.g. after PowerShell-native cmdlets).
        #[cfg(windows)]
        let cmd_line = format!(
            "{command}\r\nWrite-Host ''\r\nWrite-Host ('{sentinel}:' + $(if ($null -eq $LASTEXITCODE) {{ 0 }} else {{ $LASTEXITCODE }}))\r\n",
            command = command,
            sentinel = sentinel
        );
        // Bash/sh: $? holds exit status
        #[cfg(not(windows))]
        let cmd_line = format!(
            "{command}\nprintf '\\n{sentinel}:%d\\n' $?\n",
            command = command,
            sentinel = sentinel
        );
        {
            let mut w = self
                .master_writer
                .lock()
                .map_err(|_| anyhow!("Writer lock poisoned"))?;
            w.write_all(cmd_line.as_bytes())
                .map_err(|e| anyhow!("PTY write failed: {}", e))?;
            w.flush().map_err(|e| anyhow!("PTY flush failed: {}", e))?;
        }

        if let Some(ref mut rec) = self.recorder {
            rec.record_input(&format!("{}\n", command));
        }

        // Read output until sentinel line
        let deadline = tokio::time::Instant::now() + timeout;
        let mut output = String::new();
        let mut _exit_code = -1i32;

        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(anyhow!("Command timed out after {}s", timeout.as_secs()));
            }

            match tokio::time::timeout(remaining, self.output_rx.recv()).await {
                Ok(Some(line)) => {
                    // Check if this is the sentinel line
                    let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
                    if let Some(rest) = trimmed.strip_prefix(&sentinel) {
                        _exit_code = rest
                            .strip_prefix(':')
                            .and_then(|s| s.trim().parse().ok())
                            .unwrap_or(-1);
                        break;
                    }
                    output.push_str(&line);
                }
                Ok(None) => {
                    return Err(anyhow!("Shell session closed unexpectedly"));
                }
                Err(_) => {
                    return Err(anyhow!("Command timed out after {}s", timeout.as_secs()));
                }
            }
        }

        if let Some(ref mut rec) = self.recorder {
            rec.record_output(&output);
        }

        self.last_used = SystemTime::now();
        self.commands_run += 1;

        Ok((output, _exit_code))
    }

    /// Capture a snapshot of the current environment.
    ///
    /// This is a side-channel sentinel command — it does NOT increment `commands_run`,
    /// is NOT recorded in the audit log, and is NOT written to the asciicast recorder.
    /// Used exclusively by the dynamic discovery engine for env-diff tracking.
    ///
    /// Returns `None` on timeout or PTY error.
    pub async fn snapshot_env(&mut self) -> Option<HashMap<String, String>> {
        let seq = self.sentinel_seq.fetch_add(1, Ordering::Relaxed);
        let sentinel = format!("{}{:016x}", self.sentinel_prefix, seq);

        // PowerShell: enumerate env vars in KEY=VALUE format, then sentinel with exit code
        #[cfg(windows)]
        let cmd_line = format!(
            "Get-ChildItem env: | ForEach-Object {{ \"$($_.Name)=$($_.Value)\" }}\r\nWrite-Host ''\r\nWrite-Host ('{sentinel}:' + $(if ($null -eq $LASTEXITCODE) {{ 0 }} else {{ $LASTEXITCODE }}))\r\n",
            sentinel = sentinel
        );
        // Bash: printenv outputs KEY=VALUE per line
        #[cfg(not(windows))]
        let cmd_line = format!(
            "printenv\nprintf '\\n{sentinel}:%d\\n' $?\n",
            sentinel = sentinel
        );

        {
            let mut w = self.master_writer.lock().ok()?;
            w.write_all(cmd_line.as_bytes()).ok()?;
            w.flush().ok()?;
        }

        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut output = String::new();

        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return None;
            }
            match tokio::time::timeout(remaining, self.output_rx.recv()).await {
                Ok(Some(line)) => {
                    let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
                    if trimmed.starts_with(&sentinel) {
                        break;
                    }
                    output.push_str(&line);
                }
                _ => return None,
            }
        }

        // Parse printenv output: KEY=VALUE lines (values may contain '=')
        let mut map = HashMap::new();
        for line in output.lines() {
            if let Some(eq_pos) = line.find('=') {
                let key = line[..eq_pos].to_string();
                let value = line[eq_pos + 1..].to_string();
                if !key.is_empty() {
                    map.insert(key, value);
                }
            }
        }

        Some(map)
    }

    /// Snapshot metadata about this session.
    pub fn info(&self) -> ShellSessionInfo {
        let to_ms = |t: SystemTime| {
            t.duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0)
        };
        ShellSessionInfo {
            session_id: self.session_id.clone(),
            agent_id: self.agent_id.clone(),
            created_at_ms: to_ms(self.created_at),
            last_used_ms: to_ms(self.last_used),
            commands_run: self.commands_run,
        }
    }
}

impl Drop for ShellSession {
    fn drop(&mut self) {
        // Ask bash to exit cleanly first; ignore errors (the writer may be gone).
        if let Ok(mut w) = self.master_writer.lock() {
            #[cfg(windows)]
            let _ = w.write_all(b"exit\r\n");
            #[cfg(not(windows))]
            let _ = w.write_all(b"exit\n");
            let _ = w.flush();
        }
        // Kill + reap regardless so the process cannot outlive the session.
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
        debug!(
            "[NEXIGATE/SESSION] Session {} cleaned up (child killed and reaped)",
            &self.session_id[..8.min(self.session_id.len())]
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn make_session() -> ShellSession {
        let shell = crate::config::shell::default_shell_binary();
        ShellSession::new("test-agent", &shell, "__NEXIGATE__", &[], None, 100)
            .await
            .expect("session creation failed")
    }

    #[tokio::test]
    async fn test_session_echo() {
        let mut s = make_session().await;
        let (out, code) = s
            .run_command("echo hello", Duration::from_secs(5))
            .await
            .unwrap();
        assert!(out.contains("hello"), "output: {:?}", out);
        assert_eq!(code, 0);
    }

    #[tokio::test]
    async fn test_exit_code_captured() {
        let mut s = make_session().await;
        let fail_cmd = crate::test_utils::cmd_exit_fail();
        let (_, code) = s
            .run_command(fail_cmd, Duration::from_secs(5))
            .await
            .unwrap();
        assert_ne!(code, 0, "fail command should exit non-zero");
    }

    #[tokio::test]
    async fn test_exit_code_success() {
        let mut s = make_session().await;
        let ok_cmd = crate::test_utils::cmd_exit_ok();
        let (_, code) = s.run_command(ok_cmd, Duration::from_secs(5)).await.unwrap();
        assert_eq!(code, 0);
    }

    #[tokio::test]
    async fn test_cwd_persists() {
        let mut s = make_session().await;
        let tmp = crate::test_utils::temp_dir_string();
        let cd_cmd = format!("cd '{}'", tmp);
        s.run_command(&cd_cmd, Duration::from_secs(5))
            .await
            .unwrap();
        // pwd works in both bash and PowerShell (alias for Get-Location)
        let pwd_cmd = "pwd";
        let (out, code) = s.run_command(pwd_cmd, Duration::from_secs(5)).await.unwrap();
        assert_eq!(code, 0);
        // Normalize comparison: temp dir on macOS may resolve via /private/tmp.
        // Trim trailing slashes and whitespace/CRLF from PTY output before comparing.
        let tmp_lower = tmp.to_lowercase().replace('\\', "/");
        let tmp_compare = tmp_lower.trim_end_matches('/');
        let out_lower = out.to_lowercase().replace('\\', "/");
        let out_trimmed = out_lower.trim();
        assert!(
            out_trimmed.contains(tmp_compare) || out_trimmed.contains("/private/tmp"),
            "cwd not persisted: output={:?}, expected to contain {:?}",
            out,
            tmp
        );
    }

    #[tokio::test]
    async fn test_var_persists() {
        let mut s = make_session().await;
        let set_cmd = crate::test_utils::cmd_set_var("NEXIGATE_TEST_VAR", "bar123");
        s.run_command(&set_cmd, Duration::from_secs(5))
            .await
            .unwrap();
        let echo_cmd = crate::test_utils::cmd_echo_var("NEXIGATE_TEST_VAR");
        let (out, _) = s
            .run_command(&echo_cmd, Duration::from_secs(5))
            .await
            .unwrap();
        assert!(out.contains("bar123"), "var not persisted: {:?}", out);
    }

    #[tokio::test]
    async fn test_multiline_output() {
        let mut s = make_session().await;
        let multi_cmd = crate::test_utils::cmd_multiline_output();
        let (out, code) = s
            .run_command(multi_cmd, Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(code, 0);
        assert!(out.contains("line1"));
        assert!(out.contains("line2"));
        assert!(out.contains("line3"));
    }

    #[tokio::test]
    async fn test_timeout_fires() {
        let mut s = make_session().await;
        let sleep_cmd = crate::test_utils::cmd_sleep_long();
        let result = s.run_command(sleep_cmd, Duration::from_secs(2)).await;
        assert!(result.is_err(), "should timeout");
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("timed out"), "error: {}", msg);
    }

    #[tokio::test]
    async fn test_injected_env_visible() {
        let env = vec![("NEXIGATE_MY_VAR".to_string(), "hello_gate".to_string())];
        let shell = crate::config::shell::default_shell_binary();
        let mut s = ShellSession::new("test", &shell, "__NEXIGATE__", &env, None, 100)
            .await
            .expect("new session");
        let echo_cmd = crate::test_utils::cmd_echo_var("NEXIGATE_MY_VAR");
        let (out, _) = s
            .run_command(&echo_cmd, Duration::from_secs(5))
            .await
            .unwrap();
        assert!(out.contains("hello_gate"), "injected var: {:?}", out);
    }

    #[tokio::test]
    async fn test_sentinel_uniqueness() {
        // Two commands should never see each other's sentinels
        let mut s = make_session().await;
        let (out1, code1) = s
            .run_command("echo FIRST", Duration::from_secs(5))
            .await
            .unwrap();
        let (out2, code2) = s
            .run_command("echo SECOND", Duration::from_secs(5))
            .await
            .unwrap();
        assert!(out1.contains("FIRST"), "out1: {:?}", out1);
        assert!(
            !out1.contains("SECOND"),
            "out1 should not contain SECOND: {:?}",
            out1
        );
        assert!(out2.contains("SECOND"), "out2: {:?}", out2);
        assert_eq!(code1, 0);
        assert_eq!(code2, 0);
    }

    #[tokio::test]
    async fn test_session_info() {
        let s = make_session().await;
        let info = s.info();
        assert_eq!(info.agent_id, "test-agent");
        assert_eq!(info.commands_run, 0);
        assert!(info.created_at_ms > 0);
    }
}
