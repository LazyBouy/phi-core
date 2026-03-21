//! Search tool — grep/ripgrep-style search across files.
/*
ARCHITECTURE: SearchTool — delegate to native system tools

Rather than implementing regex search in Rust, we delegate to `rg` (ripgrep)
or `grep` — battle-tested, fast, and already installed on most systems.

Why not implement search natively?
  - ripgrep is dramatically faster for large codebases (parallel, SIMD, smart ignore rules)
  - `.gitignore` / `.ignore` support comes for free with `rg`
  - No need to maintain regex engine, file walker, and output formatting ourselves

Strategy: try `rg` first (faster, smarter), fall back to `grep -r`.
`which rg` detects availability; we check at runtime rather than compile time.

RUST QUIRK: `tokio::process::Command` — spawning system processes asynchronously
  Same pattern as BashTool: build the command, `.output().await`, parse stdout.
  The key difference is that the command is tightly scoped (specific arguments) rather
  than an arbitrary shell command — no security risk from user input in shell expansion.
*/

use crate::types::*;
use async_trait::async_trait;
use std::time::Duration;
use tokio::process::Command;

/// Search files using grep (or ripgrep if available).
pub struct SearchTool {
    /// Root directory to search in
    pub root: Option<String>,
    /// Max results to return
    pub max_results: usize,
    /// Timeout
    pub timeout: Duration,
}

impl Default for SearchTool {
    fn default() -> Self {
        Self {
            root: None,
            max_results: 50,
            timeout: Duration::from_secs(30),
        }
    }
}

impl SearchTool {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_root(mut self, root: impl Into<String>) -> Self {
        self.root = Some(root.into());
        self
    }
}

#[async_trait]
impl AgentTool for SearchTool {
    fn name(&self) -> &str {
        "search"
    }

    fn label(&self) -> &str {
        "Search Files"
    }

    fn description(&self) -> &str {
        "Search for a pattern across files using grep. Returns matching lines with file paths and line numbers. Supports regex patterns."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Search pattern (regex supported)"
                },
                "path": {
                    "type": "string",
                    "description": "Directory or file to search in (optional, defaults to working directory)"
                },
                "include": {
                    "type": "string",
                    "description": "File glob pattern to include, e.g. '*.rs' (optional)"
                },
                "case_sensitive": {
                    "type": "boolean",
                    "description": "Case sensitive search (default: false)"
                }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let cancel = ctx.cancel;
        let pattern = params["pattern"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("missing 'pattern' parameter".into()))?;

        let search_path = params["path"]
            .as_str()
            .map(|s| s.to_string())
            .or_else(|| self.root.clone())
            .unwrap_or_else(|| ".".into());

        let include = params["include"].as_str();
        let case_sensitive = params["case_sensitive"].as_bool().unwrap_or(false);

        if cancel.is_cancelled() {
            return Err(ToolError::Cancelled);
        }

        // Try ripgrep first, fall back to grep
        let (cmd_name, args) = if which_exists("rg") {
            build_rg_args(
                pattern,
                &search_path,
                include,
                case_sensitive,
                self.max_results,
            )
        } else {
            build_grep_args(
                pattern,
                &search_path,
                include,
                case_sensitive,
                self.max_results,
            )
        };

        let mut cmd = Command::new(&cmd_name);
        cmd.args(&args);
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let timeout = self.timeout;

        let result = tokio::select! {
            _ = cancel.cancelled() => {
                return Err(ToolError::Cancelled);
            }
            _ = tokio::time::sleep(timeout) => {
                return Err(ToolError::Failed("Search timed out".into()));
            }
            result = cmd.output() => {
                result.map_err(|e| ToolError::Failed(format!("Search failed: {}", e)))?
            }
        };

        let stdout = String::from_utf8_lossy(&result.stdout).to_string();
        let stderr = String::from_utf8_lossy(&result.stderr).to_string();

        // grep returns exit code 1 for "no matches" — that's not an error
        if result.status.code() == Some(2)
            || (!stderr.is_empty() && result.status.code() != Some(1))
        {
            return Err(ToolError::Failed(format!("Search error: {}", stderr)));
        }

        if stdout.trim().is_empty() {
            return Ok(ToolResult {
                content: vec![Content::Text {
                    text: format!("No matches found for '{}'", pattern),
                }],
                details: serde_json::json!({ "matches": 0 }),
            });
        }

        let match_count = stdout.lines().count();
        let text = if match_count >= self.max_results {
            format!(
                "{}\n... (showing first {} matches)",
                stdout.trim(),
                self.max_results
            )
        } else {
            format!("{}\n({} matches)", stdout.trim(), match_count)
        };

        Ok(ToolResult {
            content: vec![Content::Text { text }],
            details: serde_json::json!({ "matches": match_count }),
        })
    }
}

fn which_exists(name: &str) -> bool {
    std::process::Command::new("which")
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn build_rg_args(
    pattern: &str,         // REGEX — the search pattern passed to rg
    path: &str,            // ROOT — directory or file to search in
    include: Option<&str>, // GLOB FILTER — e.g. "*.rs"; None = search all files
    case_sensitive: bool,  // CASE — false adds --ignore-case flag
    max_results: usize,    // LIMIT — passed as --max-count to bound output size
) -> (String, Vec<String>) {
    // returns (command_name, args_vec) — caller invokes tokio::process::Command
    let mut args = vec![
        "--line-number".into(),
        "--no-heading".into(),
        format!("--max-count={}", max_results),
    ];

    if !case_sensitive {
        args.push("--ignore-case".into());
    }

    if let Some(glob) = include {
        args.push(format!("--glob={}", glob));
    }

    args.push(pattern.into());
    args.push(path.into());

    ("rg".into(), args)
}

fn build_grep_args(
    pattern: &str,         // REGEX — the search pattern passed to grep
    path: &str,            // ROOT — directory or file to search in
    include: Option<&str>, // GLOB FILTER — e.g. "*.rs"; None = search all files
    case_sensitive: bool,  // CASE — false adds -i flag
    max_results: usize,    // LIMIT — passed as -m to bound output size
) -> (String, Vec<String>) {
    // returns (command_name, args_vec)
    let mut args = vec!["-r".into(), "-n".into(), format!("-m{}", max_results)];

    if !case_sensitive {
        args.push("-i".into());
    }

    if let Some(glob) = include {
        args.push(format!("--include={}", glob));
    }

    args.push(pattern.into());
    args.push(path.into());

    ("grep".into(), args)
}
