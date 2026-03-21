//! Built-in agent tools.
/*
ARCHITECTURE: tools/ — the standard toolkit for coding agents

This module provides 6 built-in tools that together cover the core operations
of a coding agent:
  `BashTool`       — run shell commands (most powerful; the agent's hands)
  `ReadFileTool`   — read file contents
  `WriteFileTool`  — write or overwrite a file
  `EditFileTool`   — precise text replacement within a file
  `ListFilesTool`  — list directory contents
  `SearchTool`     — grep / content search across files

`default_tools()` returns all six in a `Vec<Box<dyn AgentTool>>` — the canonical
"batteries included" tool set. Callers that want a subset can build their own Vec.

RUST QUIRK: `pub mod` vs `pub use`

`pub mod bash;` — declares the `bash` submodule and makes it publicly accessible.
  This loads `tools/bash.rs` and exposes it as `tools::bash::*`.

`pub use bash::BashTool;` — re-exports `BashTool` at this module's level.
  Without this re-export, callers would write `tools::bash::BashTool`.
  With it, they write `tools::BashTool` — cleaner public API.

RUST QUIRK: `Vec<Box<dyn AgentTool>>` — heterogeneous tool collection

All 6 tools are different concrete types, but they share the `AgentTool` trait.
To put different types in one `Vec`, we need "type erasure" via trait objects:
  `Box<dyn AgentTool>` — heap-allocated, vtable-dispatched, concrete type erased

`Box::new(BashTool::default())` — allocates `BashTool` on the heap and widens
the concrete type to `Box<dyn AgentTool>`. Rust does this coercion automatically
because `BashTool: AgentTool`.
Python analogy: a list of objects that all implement an abstract base class.
*/

pub mod bash;
pub mod edit;
pub mod file;
pub mod list;
pub mod search;

pub use bash::BashTool;
pub use edit::EditFileTool;
pub use file::{ReadFileTool, WriteFileTool};
pub use list::ListFilesTool;
pub use search::SearchTool;

use crate::types::AgentTool;

/// Get the standard set of coding agent tools.
///
/// Returns all 6 built-in tools ready for use with `Agent::with_tools()` or
/// `AgentLoopConfig`. Each tool is heap-allocated as a `Box<dyn AgentTool>`.
pub fn default_tools() -> Vec<Box<dyn AgentTool>> {
    vec![
        Box::new(BashTool::default()),
        Box::new(ReadFileTool::default()),
        Box::new(WriteFileTool::new()),
        Box::new(EditFileTool::new()),
        Box::new(ListFilesTool::default()),
        Box::new(SearchTool::default()),
    ]
}
