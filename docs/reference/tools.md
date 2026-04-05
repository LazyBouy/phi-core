<!-- Last verified: 2026-04-05 by Claude Code -->
# Built-in Tools

phi-core ships with six coding-oriented tools. Get them all with `default_tools()`:

```rust
use phi_core::tools::default_tools;
let tools = default_tools();
```

## BashTool

Execute shell commands with timeout and output capture.

- **Name**: `bash`
- **Parameters**: `command` (string, required)

### Configuration

```rust
pub struct BashTool {
    pub cwd: Option<String>,           // Working directory
    pub timeout: Duration,             // Default: 120s
    pub max_output_bytes: usize,       // Default: 256KB
    pub deny_patterns: Vec<String>,    // Blocked commands
    pub confirm_fn: Option<ConfirmFn>, // Confirmation callback
}
```

Default deny patterns: `rm -rf /`, `rm -rf /*`, `mkfs`, `dd if=`, fork bomb.

### Example

```rust
let bash = BashTool::default();
// Or customize:
let bash = BashTool {
    cwd: Some("/workspace".into()),
    timeout: Duration::from_secs(60),
    ..Default::default()
};
```

## ReadFileTool

Read file contents with optional line range.

- **Name**: `read_file`
- **Parameters**: `path` (required), `offset` (optional, 1-indexed line), `limit` (optional, number of lines)

### Configuration

```rust
pub struct ReadFileTool {
    pub max_bytes: usize,              // Default: 1MB
    pub allowed_paths: Vec<String>,    // Path restrictions (empty = no restriction)
}
```

## WriteFileTool

Write content to a file. Creates parent directories automatically.

- **Name**: `write_file`
- **Parameters**: `path` (required), `content` (required)

## EditFileTool

Surgical search/replace edits. The most important tool for coding agents — instead of rewriting entire files, the agent specifies exact text to find and replace.

- **Name**: `edit_file`
- **Parameters**: `path` (required), `old_text` (required), `new_text` (required)

The `old_text` must match exactly, including whitespace and indentation.

## ListFilesTool

List files and directories with optional glob filtering.

- **Name**: `list_files`
- **Parameters**: `path` (optional, default: `.`), `pattern` (optional glob)

### Configuration

```rust
pub struct ListFilesTool {
    pub max_results: usize,    // Default: 200
    pub timeout: Duration,     // Default: 10s
}
```

Uses `find` or `fd` for efficient traversal.

## SearchTool

Search files using grep (or ripgrep if available).

- **Name**: `search`
- **Parameters**: `pattern` (required, regex), `path` (optional root directory)

### Configuration

```rust
pub struct SearchTool {
    pub root: Option<String>,      // Root directory
    pub max_results: usize,        // Default: 50
    pub timeout: Duration,         // Default: 30s
}
```

Returns matching lines with file paths and line numbers.

## PrunTool

Model-directed context pruning. Removes the oldest inrun_context entries (model-generated messages) from the working context to reclaim space in the context window. Pruned content is preserved in the session log.

- **Name**: `prun`
- **Parameters**: `tokens` (integer, required) -- approximate number of tokens to reclaim

The tool removes inrun_context entries oldest-first until the requested token budget is met. User messages are never affected. Returns a confirmation with the actual token count reclaimed.

### Configuration

```rust
let agent = BasicAgent::new(model_config)
    .with_prun_tool();  // enables both prun and prun_with_memo
```

## PrunWithMemoTool

Context pruning with a summary replacement. Same removal behavior as `prun`, but inserts a concise memo at the position of the earliest pruned message so the model retains key takeaways.

- **Name**: `prun_with_memo`
- **Parameters**: `tokens` (integer, required) -- approximate number of tokens to reclaim; `memo` (string, required) -- concise summary to retain in working context

The memo appears at the original timestamp of the earliest pruned message, preserving conversation chronology. Useful when pruned content contained decisions or conclusions worth remembering.

See [Context Pruning](../concepts/context-pruning.md) for the full design.
