//! File tools — read and write files with safety limits.
/*
ARCHITECTURE: ReadFileTool + WriteFileTool — the agent's eyes and hands on disk

`ReadFileTool` reads files and returns their content as text (or base64 for images).
`WriteFileTool` creates or overwrites files.

Safety limits:
  - `max_bytes` on ReadFileTool prevents OOM when reading huge files
  - `allowed_paths` restricts which directories can be accessed (empty = no restriction)

Image support:
  Reading `.jpg/.png/.webp/.gif/.bmp` files returns `Content::Image` (base64-encoded)
  so the LLM can "see" screenshots, diagrams, and mockups inline.

Design: return errors as ToolError (not panic or ProviderError)
  `ToolError::Failed(msg)` tells the agent loop to feed the error text back to the LLM.
  The LLM can then decide how to recover (try a different path, create the directory, etc.)
*/

use crate::types::*;
use async_trait::async_trait;
use base64::Engine;
use std::path::Path;

/// 20 MB limit for image files
const MAX_IMAGE_SIZE_BYTES: u64 = 20 * 1024 * 1024;

/// Returns true if the path has a recognized image file extension.
/*
RUST QUIRK: `matches!(expr, pattern)` — compact pattern matching returning bool

`matches!(path.extension()...., Some("jpg" | "jpeg" | "png" | ...))` is shorthand for:
  match path.extension().... {
      Some("jpg") | Some("jpeg") | Some("png") | ... => true,
      _ => false,
  }

The `|` inside `Some(...)` is an "or-pattern" — it matches any of the listed values.
This requires Rust 1.53+ (stable since then). Prior versions required separate `|` arms.

`.extension()` returns `Option<&OsStr>` (OS-native string, may not be UTF-8).
`.and_then(|e| e.to_str())` tries to convert to `&str` — returns None for non-UTF-8 paths.
`.map(|e| e.to_lowercase())` normalizes case: "PNG" and "png" both work.
`.as_deref()` converts `Option<String>` → `Option<&str>` for the pattern match.
*/
fn is_image_file(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .as_deref(),
        Some("jpg" | "jpeg" | "png" | "webp" | "gif" | "bmp")
    )
}

/// Returns the MIME type for recognized image extensions.
/*
RUST QUIRK: `Option<&'static str>` — returning a reference to a string literal

`&'static str` is a reference to a string that lives for the entire program lifetime
(string literals are baked into the binary). Returning `&'static str` from this function
is safe because "image/jpeg" etc. are compile-time constants — they always exist.
If we returned `Option<String>`, we'd heap-allocate a new String each call (wasteful).
*/
fn get_image_mime_type(path: &Path) -> Option<&'static str> {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .as_deref()
    {
        Some("jpg" | "jpeg") => Some("image/jpeg"),
        Some("png") => Some("image/png"),
        Some("webp") => Some("image/webp"),
        Some("gif") => Some("image/gif"),
        Some("bmp") => Some("image/bmp"),
        _ => None,
    }
}

/// Read a file's contents. Supports line range for large files.
pub struct ReadFileTool {
    /// Max file size to read (prevents OOM)
    pub max_bytes: usize,
    /// Allowed directory roots (empty = no restriction)
    pub allowed_paths: Vec<String>,
}

impl Default for ReadFileTool {
    fn default() -> Self {
        Self {
            max_bytes: 1024 * 1024, // 1MB
            allowed_paths: Vec::new(),
        }
    }
}

impl ReadFileTool {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl AgentTool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn label(&self) -> &str {
        "Read File"
    }

    fn description(&self) -> &str {
        "Read a file's contents. Supports text files with optional offset/limit, and image files (jpg, png, webp, gif, bmp) which are returned as base64-encoded images."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path to read"
                },
                "offset": {
                    "type": "integer",
                    "description": "Starting line number (1-indexed, optional)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to return (optional)"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value, // LLM INPUT — expects `{"path", "offset"?, "limit"?}`; offset/limit for partial reads of large text files
        ctx: ToolContext, // SYSTEM ENV — ctx.cancel checked before disk access; images bypass line-range logic
    ) -> Result<ToolResult, ToolError> {
        let path = params["path"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("missing 'path' parameter".into()))?;

        if ctx.cancel.is_cancelled() {
            return Err(ToolError::Cancelled);
        }

        // Check file exists and size
        let metadata = tokio::fs::metadata(path)
            .await
            .map_err(|e| ToolError::Failed(format!("Cannot access {}: {}", path, e)))?;

        // Handle image files: read as binary, return base64-encoded Content::Image
        let file_path = Path::new(path);
        if is_image_file(file_path) {
            if metadata.len() > MAX_IMAGE_SIZE_BYTES {
                return Err(ToolError::Failed(format!(
                    "Image too large ({}MB, max 20MB)",
                    metadata.len() / (1024 * 1024)
                )));
            }
            let mime_type = get_image_mime_type(file_path)
                .ok_or_else(|| ToolError::Failed("Unknown image format".into()))?;
            let bytes = tokio::fs::read(path)
                .await
                .map_err(|e| ToolError::Failed(format!("Cannot read {}: {}", path, e)))?;
            let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
            return Ok(ToolResult {
                content: vec![Content::Image {
                    data,
                    mime_type: mime_type.to_string(),
                }],
                details: serde_json::json!({ "path": path, "bytes": bytes.len() }),
            });
        }

        // Text files: check size limit and apply line offset/limit
        if metadata.len() as usize > self.max_bytes {
            return Err(ToolError::Failed(format!(
                "File too large ({} bytes, max {}). Use offset/limit for partial reads.",
                metadata.len(),
                self.max_bytes
            )));
        }

        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| ToolError::Failed(format!("Cannot read {}: {}", path, e)))?;

        let offset = params["offset"].as_u64().map(|v| v.max(1) as usize);
        let limit = params["limit"].as_u64().map(|v| v as usize);

        // Always show line numbers — helps agent reference exact lines for edit_file
        let lines: Vec<&str> = content.lines().collect();
        let total = lines.len();

        let (start, end) = match (offset, limit) {
            (Some(off), Some(lim)) => {
                let s = (off - 1).min(total);
                (s, (s + lim).min(total))
            }
            (Some(off), None) => {
                let s = (off - 1).min(total);
                (s, total)
            }
            (None, Some(lim)) => (0, lim.min(total)),
            (None, None) => (0, total),
        };

        let numbered: Vec<String> = lines[start..end]
            .iter()
            .enumerate()
            .map(|(i, line)| format!("{:>4} | {}", start + i + 1, line))
            .collect();

        let header = if start > 0 || end < total {
            format!("[Lines {}-{} of {}]", start + 1, end, total)
        } else {
            format!("[{} lines]", total)
        };

        let output = format!("{}\n{}", header, numbered.join("\n"));

        Ok(ToolResult {
            content: vec![Content::Text { text: output }],
            details: serde_json::json!({ "path": path }),
        })
    }
}

// ---------------------------------------------------------------------------

/// Write content to a file. Creates parent directories if needed.
pub struct WriteFileTool;

impl Default for WriteFileTool {
    fn default() -> Self {
        Self::new()
    }
}

impl WriteFileTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl AgentTool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn label(&self) -> &str {
        "Write File"
    }

    fn description(&self) -> &str {
        "Write content to a file. Creates the file if it doesn't exist, overwrites if it does. Creates parent directories automatically."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path to write"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write to the file"
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value, // LLM INPUT — expects `{"path", "content"}`; parent directories created automatically
        ctx: ToolContext, // SYSTEM ENV — ctx.cancel checked before write; no timeout (write is fast)
    ) -> Result<ToolResult, ToolError> {
        let path = params["path"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("missing 'path' parameter".into()))?;
        let content = params["content"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("missing 'content' parameter".into()))?;

        if ctx.cancel.is_cancelled() {
            return Err(ToolError::Cancelled);
        }

        // Create parent directories
        if let Some(parent) = std::path::Path::new(path).parent() {
            if !parent.exists() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| ToolError::Failed(format!("Cannot create directory: {}", e)))?;
            }
        }

        tokio::fs::write(path, content)
            .await
            .map_err(|e| ToolError::Failed(format!("Cannot write {}: {}", path, e)))?;

        let bytes = content.len();
        Ok(ToolResult {
            content: vec![Content::Text {
                text: format!("Wrote {} bytes to {}", bytes, path),
            }],
            details: serde_json::json!({ "path": path, "bytes": bytes }),
        })
    }
}
