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

`default_tools()` returns all six in a `Vec<Arc<dyn AgentTool>>` — the canonical
"batteries included" tool set. Callers that want a subset can build their own Vec.

RUST QUIRK: `pub mod` vs `pub use`

`pub mod bash;` — declares the `bash` submodule and makes it publicly accessible.
  This loads `tools/bash.rs` and exposes it as `tools::bash::*`.

`pub use bash::BashTool;` — re-exports `BashTool` at this module's level.
  Without this re-export, callers would write `tools::bash::BashTool`.
  With it, they write `tools::BashTool` — cleaner public API.

RUST QUIRK: `Vec<Arc<dyn AgentTool>>` — shared heterogeneous tool collection

All 6 tools are different concrete types, but they share the `AgentTool` trait.
To put different types in one `Vec`, we need "type erasure" via trait objects:
  `Arc<dyn AgentTool>` — reference-counted, vtable-dispatched, concrete type erased

`Arc::new(BashTool::default())` — allocates `BashTool` on the heap behind an Arc.
Arc allows tools to be shared across parallel agent branches (evaluational parallelism)
without copying — each branch gets a cheap reference-count increment.
Python analogy: a list of objects that all implement an abstract base class.
*/

pub mod bash;
pub mod edit;
pub mod file;
pub mod list;
pub mod registry;
pub mod search;

pub use bash::BashTool;
pub use edit::EditFileTool;
pub use file::{ReadFileTool, WriteFileTool};
pub use list::ListFilesTool;
pub use registry::ToolRegistry;
pub use search::SearchTool;

use crate::types::AgentTool;
use std::sync::Arc;

/// Get the standard set of coding agent tools.
///
/// Returns all 6 built-in tools ready for use with `Agent::with_tools()` or
/// `AgentLoopConfig`. Each tool is heap-allocated behind an `Arc<dyn AgentTool>`,
/// which allows them to be shared across parallel agent branches at zero copy cost.
pub fn default_tools() -> Vec<Arc<dyn AgentTool>> {
    vec![
        Arc::new(BashTool::default()),
        Arc::new(ReadFileTool::default()),
        Arc::new(WriteFileTool::new()),
        Arc::new(EditFileTool::new()),
        Arc::new(ListFilesTool::default()),
        Arc::new(SearchTool::default()),
    ]
}
