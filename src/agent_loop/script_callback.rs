//! Script-based callbacks — run shell or Python scripts as lifecycle hooks.
//!
//! `ScriptCallback` wraps an external script that receives JSON on stdin and
//! returns JSON on stdout. The builder uses this to bridge config-specified
//! script paths into Rust closure callbacks.
//!
//! # Protocol
//!
//! **Input (stdin):**
//! ```json
//! { "hook": "before_loop", "session_id": "...", "loop_id": "...", ... }
//! ```
//!
//! **Output (stdout) for `before_*` hooks:**
//! ```json
//! { "allow": true }
//! ```
//!
//! **Output (stdout) for `after_*` hooks:**
//! Any JSON (logged but not acted upon).

use std::path::{Path, PathBuf};
use std::process::Command;

/// A callback that executes an external script (shell or Python).
#[derive(Debug, Clone)]
pub struct ScriptCallback {
    /// Path to the script (.sh, .py, or any executable).
    pub path: PathBuf,
    /// Working directory for script execution.
    pub working_dir: Option<PathBuf>,
}

impl ScriptCallback {
    /// Create a new script callback.
    pub fn new(path: impl Into<PathBuf>, working_dir: Option<PathBuf>) -> Self {
        Self {
            path: path.into(),
            working_dir,
        }
    }

    /// Execute the script synchronously with JSON input on stdin.
    /// Returns the parsed JSON output from stdout.
    pub fn execute_sync(
        &self,
        input: &serde_json::Value,
    ) -> Result<serde_json::Value, ScriptCallbackError> {
        let input_json = serde_json::to_string(input)
            .map_err(|e| ScriptCallbackError::Serialization(e.to_string()))?;

        let interpreter = detect_interpreter(&self.path);
        let mut cmd = Command::new(&interpreter[0]);
        for arg in &interpreter[1..] {
            cmd.arg(arg);
        }
        cmd.arg(&self.path);

        if let Some(ref dir) = self.working_dir {
            cmd.current_dir(dir);
        }

        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| ScriptCallbackError::Spawn {
            path: self.path.display().to_string(),
            error: e.to_string(),
        })?;

        // Write input to stdin
        if let Some(ref mut stdin) = child.stdin.take() {
            use std::io::Write;
            let _ = stdin.write_all(input_json.as_bytes());
        }

        let output = child
            .wait_with_output()
            .map_err(|e| ScriptCallbackError::Execution {
                path: self.path.display().to_string(),
                error: e.to_string(),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ScriptCallbackError::NonZeroExit {
                path: self.path.display().to_string(),
                code: output.status.code(),
                stderr: stderr.to_string(),
            });
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        serde_json::from_str(stdout.trim())
            .map_err(|e| ScriptCallbackError::OutputParse(e.to_string()))
    }
}

/// Detect the interpreter for a script based on file extension.
///
/// Returns the argv prefix to spawn the appropriate interpreter for a script
/// at `path`:
///
/// - `.py` → `["python3"]`
/// - `.sh` → `["sh"]`
/// - any other extension (or none) → `["sh"]` (default to shell)
///
/// Public so downstream consumers (e.g. i-phi's hook dispatcher) can adopt the
/// same script-extension dispatch table phi-core uses internally for
/// [`ScriptCallback`], rather than re-deriving it.
pub fn detect_interpreter(path: &Path) -> Vec<String> {
    match path.extension().and_then(|e| e.to_str()) {
        Some("py") => vec!["python3".into()],
        Some("sh") => vec!["sh".into()],
        _ => vec!["sh".into()], // default to shell
    }
}

/// Returns true if a string looks like a script path (for config detection).
pub fn is_script_path(s: &str) -> bool {
    s.ends_with(".sh") || s.ends_with(".py") || s.contains('/')
}

/// Errors from script callback execution.
#[derive(Debug)]
pub enum ScriptCallbackError {
    Spawn {
        path: String,
        error: String,
    },
    Execution {
        path: String,
        error: String,
    },
    NonZeroExit {
        path: String,
        code: Option<i32>,
        stderr: String,
    },
    OutputParse(String),
    Serialization(String),
}

impl std::fmt::Display for ScriptCallbackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn { path, error } => write!(f, "Failed to spawn script {path}: {error}"),
            Self::Execution { path, error } => {
                write!(f, "Script execution failed {path}: {error}")
            }
            Self::NonZeroExit { path, code, stderr } => {
                write!(f, "Script {path} exited with code {code:?}: {stderr}")
            }
            Self::OutputParse(e) => write!(f, "Failed to parse script output as JSON: {e}"),
            Self::Serialization(e) => write!(f, "Failed to serialize input: {e}"),
        }
    }
}

impl std::error::Error for ScriptCallbackError {}
