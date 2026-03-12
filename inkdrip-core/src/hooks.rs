//! External command hook system for advanced extensibility.
//!
//! Hooks run external commands at key pipeline stages, communicating via
//! JSON over stdin/stdout.  A hook that exits non-zero or produces invalid
//! output is treated as a no-op (the original data is kept) so that a
//! misbehaving script never corrupts the pipeline.

use std::io::{BufReader, Read as _, Write as _};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::config::HookEntryConfig;
use crate::error::{InkDripError, Result};

/// Default timeout applied when none is specified.
const DEFAULT_TIMEOUT_SECS: u64 = 30;

// ─── Public helpers ─────────────────────────────────────────────

/// Run a hook command, sending `input_json` on stdin and returning the
/// parsed stdout on success.
///
/// Returns `Ok(None)` when:
/// - the hook is disabled (`enabled = false`)
/// - the command is empty
/// - the process exits non-zero (logged as a warning)
/// - stdout is empty (hook chose not to modify anything)
/// - stdout is not valid JSON of the expected type
///
/// # Errors
///
/// Returns `Err` only for truly unrecoverable situations such as JSON
/// serialization failures.  Most hook failures are gracefully swallowed
/// and return `Ok(None)`.
pub fn run_hook<I: Serialize, O: for<'de> Deserialize<'de>>(
    hook_name: &str,
    entry: &HookEntryConfig,
    input: &I,
    timeout_secs: u64,
) -> Result<Option<O>> {
    if !entry.enabled || entry.command.is_empty() {
        return Ok(None);
    }

    let input_json = serde_json::to_string(input).map_err(|e| {
        InkDripError::Other(anyhow::anyhow!(
            "hook {hook_name}: failed to serialize input: {e}"
        ))
    })?;

    let timeout = Duration::from_secs(if timeout_secs > 0 {
        timeout_secs
    } else {
        DEFAULT_TIMEOUT_SECS
    });

    let raw = match invoke_command(&entry.command, &input_json, timeout) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("hook {hook_name}: {e}");
            return Ok(None);
        }
    };

    Ok(parse_hook_output(hook_name, &raw))
}

/// Deserialize hook stdout, returning `None` for empty or invalid output.
fn parse_hook_output<O: for<'de> Deserialize<'de>>(hook_name: &str, raw: &str) -> Option<O> {
    if raw.is_empty() {
        tracing::debug!("hook {hook_name}: empty stdout, keeping original");
        return None;
    }
    match serde_json::from_str::<O>(raw) {
        Ok(parsed) => {
            tracing::debug!("hook {hook_name}: success");
            Some(parsed)
        }
        Err(e) => {
            tracing::warn!("hook {hook_name}: invalid JSON output: {e}");
            None
        }
    }
}

// ─── Internals ──────────────────────────────────────────────────

/// Spawn the command, pipe JSON in, and collect stdout with a timeout.
///
/// The command string is split on whitespace to separate the program from
/// its arguments (shell-free invocation to avoid injection risks).
///
/// Uses `try_wait` polling so the child can be killed cleanly when the
/// deadline is reached, preventing zombie processes.
fn invoke_command(command: &str, stdin_data: &str, timeout: Duration) -> Result<String> {
    let parts: Vec<&str> = command.split_whitespace().collect();
    let (program, args) = parts
        .split_first()
        .ok_or_else(|| InkDripError::ConfigError("hook command is empty".into()))?;

    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| InkDripError::Other(anyhow::anyhow!("failed to spawn hook: {e}")))?;

    // Write input to stdin and close it to signal EOF.
    if let Some(mut stdin) = child.stdin.take() {
        // Ignore write errors — the child may have already exited.
        let _ = stdin.write_all(stdin_data.as_bytes());
    }

    // Drain stdout/stderr concurrently to avoid child process blocking on full pipe buffers.
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| InkDripError::Other(anyhow::anyhow!("failed to capture hook stdout")))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| InkDripError::Other(anyhow::anyhow!("failed to capture hook stderr")))?;

    let stdout_reader = thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut buf = Vec::new();
        let result = reader.read_to_end(&mut buf);
        (result, buf)
    });
    let stderr_reader = thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut buf = Vec::new();
        let result = reader.read_to_end(&mut buf);
        (result, buf)
    });

    // Poll for completion, enforcing the deadline.
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|e| InkDripError::Other(anyhow::anyhow!("hook wait error: {e}")))?
        {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait(); // reap to avoid zombie
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(InkDripError::Other(anyhow::anyhow!(
                "hook timed out after {}s",
                timeout.as_secs()
            )));
        }
        thread::sleep(Duration::from_millis(50));
    };

    let (stdout_read, stdout_buf) = stdout_reader.join().map_err(|e| {
        InkDripError::Other(anyhow::anyhow!("hook stdout reader thread panicked: {e:?}"))
    })?;
    let (stderr_read, stderr_buf) = stderr_reader.join().map_err(|e| {
        InkDripError::Other(anyhow::anyhow!("hook stderr reader thread panicked: {e:?}"))
    })?;

    stdout_read.map_err(|e| InkDripError::Other(anyhow::anyhow!("hook stdout read error: {e}")))?;
    stderr_read.map_err(|e| InkDripError::Other(anyhow::anyhow!("hook stderr read error: {e}")))?;

    // Log stderr for debugging.
    let stderr = String::from_utf8_lossy(&stderr_buf);
    if !stderr.is_empty() {
        tracing::debug!("hook stderr: {stderr}");
    }

    if !status.success() {
        return Err(InkDripError::Other(anyhow::anyhow!(
            "hook exited with status {status}"
        )));
    }

    String::from_utf8(stdout_buf)
        .map_err(|e| InkDripError::Other(anyhow::anyhow!("hook stdout is not UTF-8: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HookEntryConfig;

    #[test]
    fn disabled_hook_returns_none() {
        let entry = HookEntryConfig {
            enabled: false,
            command: "echo hello".into(),
        };
        let result: Result<Option<serde_json::Value>> =
            run_hook("test", &entry, &serde_json::json!({}), 5);
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn empty_command_returns_none() {
        let entry = HookEntryConfig {
            enabled: true,
            command: String::new(),
        };
        let result: Result<Option<serde_json::Value>> =
            run_hook("test", &entry, &serde_json::json!({}), 5);
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn echo_hook_returns_json() {
        // `echo` outputs the literal string; we pass valid JSON as the argument.
        let entry = HookEntryConfig {
            enabled: true,
            command: r#"echo {"ok":true}"#.into(),
        };
        let result: Result<Option<serde_json::Value>> =
            run_hook("test", &entry, &serde_json::json!({}), 5);
        let output = result.unwrap().unwrap();
        assert_eq!(output["ok"], true);
    }

    #[test]
    fn failing_command_returns_none() {
        let entry = HookEntryConfig {
            enabled: true,
            command: "false".into(), // exits with 1
        };
        let result: Result<Option<serde_json::Value>> =
            run_hook("test", &entry, &serde_json::json!({}), 5);
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn large_stdout_does_not_deadlock() {
        let output = invoke_command("seq 1 70000", "{}", Duration::from_secs(5)).unwrap();
        assert!(output.starts_with("1\n"));
        assert!(output.contains("70000\n"));
    }
}
