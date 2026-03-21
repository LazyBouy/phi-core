//! Bash tool — execute shell commands with timeout and output capture.
/*
ARCHITECTURE: BashTool — the agent's most powerful (and most dangerous) capability

BashTool lets the agent run arbitrary shell commands. It's the tool that makes
an agent a "coding agent" rather than a "chat agent." Combined with file tools,
the agent can read code, run tests, install packages, and check system state.

Safety layers:
  1. `deny_patterns` — blocklist of dangerous command substrings; checked before execution
  2. `confirm_fn` — optional callback that asks the user to approve a command
  3. `timeout` — kills commands that run too long (prevents runaway processes)
  4. `max_output_bytes` — truncates huge outputs (prevents OOM from `cat /dev/urandom`)
  5. CancellationToken — user can interrupt a running command

Design decision: return output even on non-zero exit codes.
  The agent loop always gets stdout/stderr back even if the command failed.
  This is crucial for self-correction: the agent sees "make: command not found"
  and can decide to install `make` or use an alternative. If we returned an error,
  the agent would have no information about what went wrong.

RUST QUIRK: `tokio::process::Command` — async subprocess execution
  `tokio::process::Command` is the async version of `std::process::Command`.
  `cmd.output().await` runs the command and collects all output asynchronously.
  Unlike `std::process::Command::output()` (blocks the OS thread), the tokio
  version yields back to the runtime while waiting, allowing other tasks to run.
*/

use crate::types::*;

/// Type alias for command confirmation callback.
/*
RUST QUIRK: `type ConfirmFn = Box<dyn Fn(&str) -> bool + Send + Sync>;`

`type` creates a type alias — a shorthand name for a complex type.
`Box<dyn Fn(&str) -> bool + Send + Sync>` means:
  - A heap-allocated function (closure or fn pointer)
  - That takes a `&str` (the command being run)
  - Returns `bool` (true = allow, false = deny)
  - Is `Send + Sync` so it can be called from any thread

Why `Box<dyn Fn>`? Closures that capture variables are all different types,
so we can't use a generic `<F: Fn>` in the struct field. We erase the type
into a trait object instead.
Python analogy: `ConfirmFn = Callable[[str], bool]`
*/
pub type ConfirmFn = Box<dyn Fn(&str) -> bool + Send + Sync>;
use async_trait::async_trait;
use std::time::Duration;
use tokio::process::Command;

/// Execute shell commands. Captures stdout + stderr.
pub struct BashTool {
    /// Working directory for commands (None = inherit from current process)
    pub cwd: Option<String>,
    /// Max execution time per command (default: 120s)
    pub timeout: Duration,
    /// Max output bytes to capture (prevents OOM on huge outputs, default: 256KB)
    pub max_output_bytes: usize,
    /// Commands/patterns that are always blocked (e.g., "rm -rf /")
    pub deny_patterns: Vec<String>,
    /// Optional callback for confirming dangerous commands (None = auto-allow)
    pub confirm_fn: Option<ConfirmFn>,
}

impl Default for BashTool {
    fn default() -> Self {
        Self {
            cwd: None,
            timeout: Duration::from_secs(120),
            max_output_bytes: 256 * 1024, // 256KB
            deny_patterns: vec![
                "rm -rf /".into(),
                "rm -rf /*".into(),
                "mkfs".into(),
                "dd if=".into(),
                ":(){:|:&};:".into(), // fork bomb
            ],
            confirm_fn: None,
        }
    }
}

impl BashTool {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_cwd(mut self, cwd: impl Into<String>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_deny_patterns(mut self, patterns: Vec<String>) -> Self {
        self.deny_patterns = patterns;
        self
    }

    /*
    RUST QUIRK: `impl Fn(&str) -> bool + Send + Sync + 'static` — accepting closures generically

    This method accepts ANY callable that matches the signature `fn(&str) -> bool`.
    `impl Fn(...)` means "some type that implements the Fn trait" — the compiler
    generates a monomorphized version for each concrete closure type passed in.
    This is more efficient than `Box<dyn Fn>` (no heap allocation at call site).

    `+ Send + Sync + 'static` — required because we then `Box::new(f)` it and store it.
    The stored `Box<dyn Fn>` needs to be `Send + Sync + 'static` to be held in the struct
    (which may be shared across threads via `Arc`).
    `'static` means the closure must not capture any borrowed references.

    Python analogy: accepting a callable with `def with_confirm(self, f: Callable[[str], bool]):`
    */
    pub fn with_confirm(mut self, f: impl Fn(&str) -> bool + Send + Sync + 'static) -> Self {
        self.confirm_fn = Some(Box::new(f));
        self
    }
}

#[async_trait]
impl AgentTool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn label(&self) -> &str {
        "Execute Command"
    }

    fn description(&self) -> &str {
        "Execute a bash command and return stdout/stderr. Use for running scripts, installing packages, checking system state, etc."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The bash command to execute"
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value, // LLM INPUT — expects `{"command": "..."}` — the shell command to run
        ctx: ToolContext, // SYSTEM ENV — ctx.cancel used in tokio::select! to race cancel|timeout|execution
    ) -> Result<ToolResult, ToolError> {
        let cancel = ctx.cancel;
        let command = params["command"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("missing 'command' parameter".into()))?;

        // Check deny patterns
        for pattern in &self.deny_patterns {
            if command.contains(pattern.as_str()) {
                return Err(ToolError::Failed(format!(
                    "Command blocked by safety policy: contains '{}'. This pattern is denied for safety.",
                    pattern
                )));
            }
        }

        // Check confirmation callback
        if let Some(ref confirm) = self.confirm_fn {
            if !confirm(command) {
                return Err(ToolError::Failed(
                    "Command was not confirmed by the user.".into(),
                ));
            }
        }

        /*
        RUST QUIRK: `tokio::process::Command` — building an async subprocess

        `Command::new("bash")` creates a command builder (not yet executed).
        `.arg("-c")` adds the "-c" flag (run the next argument as a shell script).
        `.arg(command)` adds the actual command string.
        `.stdout(Stdio::piped())` — capture stdout instead of inheriting from parent.
        `.stderr(Stdio::piped())` — capture stderr too.
        `.current_dir(cwd)` — set the working directory.

        None of these actually run the process. `.output().await` launches it.
        */
        let mut cmd = Command::new("bash");
        cmd.arg("-c").arg(command);

        if let Some(ref cwd) = self.cwd {
            cmd.current_dir(cwd);
        }

        // Capture both stdout and stderr (not inherit from parent process)
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let timeout = self.timeout;
        let max_bytes = self.max_output_bytes;

        /*
        ARCHITECTURE: Three-way race: cancellation | timeout | execution

        `tokio::select!` races three futures simultaneously:
          1. `cancel.cancelled()` — user interrupted (Ctrl-C or agent stopped)
          2. `tokio::time::sleep(timeout)` — command exceeded time limit
          3. `cmd.output()` — command completed (success or failure)

        The first branch to complete wins; the others are dropped (cancelled).
        This is the idiomatic tokio pattern for "run this with a deadline."

        RUST QUIRK: `result.map_err(|e| ToolError::Failed(...))?`
          `cmd.output()` returns `Result<Output, io::Error>`.
          `.map_err(...)` converts `io::Error` → `ToolError::Failed(String)`.
          `?` propagates the error out of the whole `execute()` function if present.
        */
        let result = tokio::select! {
            _ = cancel.cancelled() => {
                return Err(ToolError::Cancelled);
            }
            _ = tokio::time::sleep(timeout) => {
                return Err(ToolError::Failed(format!(
                    "Command timed out after {}s",
                    timeout.as_secs()
                )));
            }
            result = cmd.output() => {
                result.map_err(|e| ToolError::Failed(format!("Failed to execute: {}", e)))?
            }
        };

        /*
        RUST QUIRK: `String::from_utf8_lossy(&bytes).to_string()`
          `result.stdout` is `Vec<u8>` — raw bytes.
          `from_utf8_lossy` converts bytes to `Cow<str>`:
            - If valid UTF-8 → `Cow::Borrowed(&str)` (no allocation)
            - If invalid UTF-8 → `Cow::Owned(String)` with `?` replacing bad bytes
          `.to_string()` converts the `Cow<str>` to an owned `String` in all cases.
          This handles programs that output non-UTF-8 (binary data, legacy encodings)
          gracefully — we show them with replacement characters rather than panicking.
        */
        let mut stdout = String::from_utf8_lossy(&result.stdout).to_string();
        let mut stderr = String::from_utf8_lossy(&result.stderr).to_string();

        // Truncate if too large
        if stdout.len() > max_bytes {
            stdout.truncate(max_bytes);
            stdout.push_str("\n... (output truncated)");
        }
        if stderr.len() > max_bytes {
            stderr.truncate(max_bytes);
            stderr.push_str("\n... (output truncated)");
        }

        let exit_code = result.status.code().unwrap_or(-1);

        let output = if stderr.is_empty() {
            format!("Exit code: {}\n{}", exit_code, stdout)
        } else {
            format!(
                "Exit code: {}\nSTDOUT:\n{}\nSTDERR:\n{}",
                exit_code, stdout, stderr
            )
        };

        // Return output even on failure — LLMs need error output to self-correct
        Ok(ToolResult {
            content: vec![Content::Text { text: output }],
            details: serde_json::json!({ "exit_code": exit_code, "success": exit_code == 0 }),
        })
    }
}
